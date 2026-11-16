//! `OpcUaServer` 的 [`NorthServer`] 实现（启动 / 停止 / 点位变更回调）。
//!
//! 从 `opcua_server.rs` 拆出，只为控制单文件行数（C24 行数预算），行为不变。

use super::*;

impl NorthServer for OpcUaServer {
    fn name(&self) -> &'static str {
        "opcua"
    }

    fn start<'a>(&'a self) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send + 'a>> {
        Box::pin(async move {
            if !self.config.enabled {
                return Err(GatewayError::Config("opcua server not enabled".into()));
            }

            let (host, port) = parse_endpoint(&self.config.endpoint)?;

            // 提取证书路径到本地变量（避免 async block 持有 &self 导致 !Send）
            let cert_paths = self.cert_paths.clone();
            let users = self.config.users.clone();
            let trust_client_certs = self.config.trust_client_certs;

            // 显式配置的证书缺失时拒绝启动（避免静默退化为临时自签名证书，
            // 使客户端固定信任的证书指纹与真实服务端不一致）。
            if let Some((cert_path, key_path)) = &cert_paths {
                for p in [cert_path, key_path] {
                    if !p.exists() {
                        return Err(GatewayError::Config(format!(
                            "opcua 证书文件不存在: {}",
                            p.display()
                        )));
                    }
                }
            }

            // 端点支持的 user token id：用户名列表 + （按配置/旧行为）匿名
            let token_ids = user_token_ids(&users, self.config.allow_anonymous);
            let token_refs: Vec<&str> = token_ids.iter().map(String::as_str).collect();

            // 构建 OPC UA Server（放在内部块中，确保 builder 在 await 前 drop）
            let (server, handle) = {
                let mut builder = ServerBuilder::new()
                    .application_name("Gateway OPC UA Server")
                    .application_uri("urn:gateway:opcua")
                    .product_uri("urn:gateway:opcua")
                    .host(&host)
                    .port(port)
                    // discovery_urls 必须非空：async-opcua 的 ServerConfig::validate() 会对空值
                    // 直接报 “Discovery urls not set” 使 build() 失败（服务无法启动）。
                    // 用配置的 endpoint 拼出服务地址，与 e2e 测试桩保持一致。
                    .discovery_urls(vec![format!("opc.tcp://{}:{}/", host, port)]);

                // 匿名/明文 None 端点仅在「未配置证书」或「显式 allow_anonymous」时注册：
                // 一旦配置了证书，默认只暴露 Basic256Sha256/SignAndEncrypt，避免同时留下
                // 一个无需凭据、无加密的读取通道（任何可达该端口者都能浏览过程数据）。
                let has_cert = cert_paths.is_some();
                if !users.is_empty() && !has_cert {
                    tracing::warn!(
                        "opcua: 已配置用户名/密码但未配置证书，凭据会在 None 端点上以明文传输；生产环境请配置 cert_path/key_path"
                    );
                }
                if should_register_anonymous_none_endpoint(has_cert, self.config.allow_anonymous) {
                    builder = builder.add_endpoint(
                        "none",
                        (
                            "/",
                            SecurityPolicy::None,
                            MessageSecurityMode::None,
                            token_refs.as_slice(),
                        ),
                    );
                    if has_cert {
                        tracing::warn!(
                            "opcua: allow_anonymous=true，已配证书仍注册明文匿名 None 端点（生产环境不建议）"
                        );
                    } else {
                        tracing::warn!(
                            "opcua: 未配置证书，已注册明文匿名 None 端点（仅建议本地调试/引导使用）"
                        );
                    }
                }

                // 证书：有持久化证书则使用，否则用临时自签名证书
                if let Some((cert_path, key_path)) = cert_paths {
                    builder = builder
                        .certificate_path(cert_path)
                        .private_key_path(key_path);
                    // 启用签名加密 endpoint（需要证书）
                    builder = builder.add_endpoint(
                        "sign_encrypt",
                        (
                            "/",
                            SecurityPolicy::Basic256Sha256,
                            MessageSecurityMode::SignAndEncrypt,
                            token_refs.as_slice(),
                        ),
                    );
                    tracing::info!("opcua: using persistent cert, Basic256Sha256 endpoint enabled");
                } else {
                    builder = builder.create_sample_keypair(true);
                }

                // 注册用户名/密码 token（默认认证器按 endpoint.user_token_ids
                // 中的 token id 逐一比对用户名与密码）
                for (user, password) in &users {
                    builder = builder.add_user_token(
                        user.clone(),
                        ServerUserToken::user_pass(user.clone(), password.clone()),
                    );
                }
                if !users.is_empty() {
                    tracing::info!(
                        "opcua: {} username token(s) registered (anonymous={})",
                        users.len(),
                        token_ids.iter().any(|id| id == ANONYMOUS_USER_TOKEN_ID)
                    );
                }

                builder
                    .trust_client_certs(trust_client_certs)
                    .with_node_manager(simple_node_manager(
                        NamespaceMetadata {
                            namespace_uri: GATEWAY_NAMESPACE_URI.to_string(),
                            ..Default::default()
                        },
                        "GatewayNodeManager",
                    ))
                    .build()
                    .map_err(|e| {
                        GatewayError::Protocol(format!("opcua server build failed: {}", e))
                    })?
            };

            // 获取 namespace index
            let ns = handle
                .get_namespace_index(GATEWAY_NAMESPACE_URI)
                .ok_or_else(|| {
                    GatewayError::NotFound("opcua gateway namespace not found".to_string())
                })?;
            *self.namespace_index.lock() = Some(ns);

            // 保存 handle 供 update_value 使用
            *self.handle.lock() = Some(handle.clone());

            // f3 修复：每次 start() 都会重建 Server 与全新的地址空间，上一次运行留下的
            // NodeId 已失效。必须清空映射表，否则 ensure_node_in_address_space 命中陈旧
            // NodeId 后直接早退，后续 set_value 写到一个不存在的节点上（静默失败），
            // OPC UA 订阅者永远收不到重启后的数据更新。
            self.point_to_node.write().clear();
            // 地址空间同样是全新的：内存节点缓存一并清空，否则 node_count 会把上一次
            // 运行的节点算进来（节点要等下一次 update 时才会重建）。
            self.nodes.write().clear();

            // 创建 Gateway folder 节点（Objects/Gateway）
            let gateway_folder = NodeId::new(ns, "Gateway");
            if let Some(nm) = handle.node_managers().get_of_type::<SimpleNodeManager>() {
                let mut aspace = nm.address_space().write();
                ObjectBuilder::new(&gateway_folder, "Gateway", "Gateway")
                    .organized_by(ObjectId::ObjectsFolder)
                    .insert(&mut *aspace);
            }

            self.running.store(true, Ordering::Relaxed);
            tracing::info!(
                "opcua server started on opc.tcp://{}:{} (namespace index={})",
                host,
                port,
                ns
            );

            // 运行 server（阻塞直到 cancel 或出错）
            let result = server.run().await;

            self.running.store(false, Ordering::Relaxed);
            *self.handle.lock() = None;
            *self.namespace_index.lock() = None;

            result.map_err(|e| GatewayError::Protocol(format!("opcua server error: {}", e)))
        })
    }

    fn stop<'a>(&'a self) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(handle) = self.handle.lock().take() {
                handle.cancel();
                tracing::info!("opcua server stop signal sent");
            }
            self.running.store(false, Ordering::Relaxed);
            Ok(())
        })
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn on_points_changed(&self, active: &ActivePointSet) {
        // 先记录配置口径：此后懒建的节点按活跃集决定初始写权限。
        *self.active_points.write() = Some(active.clone());

        // 取 handle 快照后立即释放锁（临时 guard 在 let 语句结束时 drop）。
        let handle = match self.handle.lock().as_ref() {
            Some(h) => h.clone(),
            // 未启动：地址空间尚不存在，节点会在 start 后按活跃集懒建。
            None => return,
        };
        let Some(nm) = handle.node_managers().get_of_type::<SimpleNodeManager>() else {
            return;
        };
        self.sync_address_space(&nm, active);
    }
}
