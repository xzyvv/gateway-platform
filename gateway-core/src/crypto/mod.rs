//! 加密相关模块
//!
//! 子模块：
//! - base64：Base64 编解码（内部用）
//! - sha256：SHA256 摘要算法（暴露 sha256_bytes 给模块外用）
//! - password：PBKDF2 密码哈希（Web 登录密码）
//! - secret：配置敏感字段加密（机器绑定密钥）
//! - cert：TLS 证书管理

pub mod cert;
pub mod password;
pub mod secret;
pub mod sha256;

// 重导出 pub API，外部调用 `crate::crypto::hash_password` 等无需改动
pub use password::{generate_api_token, hash_password, verify_password};
pub use secret::{
    decrypt, decrypt_config_secrets, encrypt, encrypt_config_secrets, is_encrypted, try_encrypt,
};
pub use sha256::sha256_bytes;
