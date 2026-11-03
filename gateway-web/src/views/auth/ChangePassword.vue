<template>
  <div class="cp-container">
    <div class="cp-bg-shape shape-1"></div>
    <div class="cp-bg-shape shape-2"></div>
    <el-card class="cp-card">
      <div class="cp-logo">
        <el-icon :size="36"><Lock /></el-icon>
      </div>
      <div class="cp-header">
        <h1>修改密码</h1>
        <p>请输入当前密码与新密码</p>
      </div>
      <el-form ref="formRef" :model="form" :rules="rules" @submit.prevent="handleSubmit">
        <el-form-item prop="current">
          <el-input
            v-model="form.current"
            type="password"
            placeholder="当前密码"
            aria-label="当前密码"
            size="large"
            show-password
            @keyup.enter="handleSubmit"
          >
            <template #prefix><el-icon><Key /></el-icon></template>
          </el-input>
        </el-form-item>
        <el-form-item prop="new">
          <el-input
            v-model="form.new"
            type="password"
            placeholder="新密码（至少 6 位）"
            aria-label="新密码"
            size="large"
            show-password
            @keyup.enter="handleSubmit"
          >
            <template #prefix><el-icon><Lock /></el-icon></template>
          </el-input>
          <!-- 密码强度提示：长度 + 字符多样性 -->
          <div v-if="form.new" class="strength-bar" :title="strengthLabel">
            <div class="strength-track">
              <div
                class="strength-fill"
                :class="`is-${strengthLevel}`"
                :style="{ width: strengthPercent }"
              ></div>
            </div>
            <span class="strength-label" :class="`is-${strengthLevel}`">
              {{ strengthLabel }}
            </span>
          </div>
        </el-form-item>
        <el-form-item prop="confirm">
          <el-input
            v-model="form.confirm"
            type="password"
            placeholder="再次输入新密码"
            aria-label="确认新密码"
            size="large"
            show-password
            @keyup.enter="handleSubmit"
          >
            <template #prefix><el-icon><CircleCheck /></el-icon></template>
          </el-input>
        </el-form-item>
        <el-button
          type="primary"
          size="large"
          class="cp-btn"
          :loading="loading"
          :disabled="loading"
          @click="handleSubmit"
        >
          <el-icon v-if="!loading"><Check /></el-icon>
          <span>{{ loading ? '提交中' : '确认修改' }}</span>
        </el-button>
        <el-button
          size="large"
          class="cp-cancel"
          :disabled="loading"
          @click="goBack"
        >
          <el-icon><Back /></el-icon>
          <span>返回</span>
        </el-button>
      </el-form>
      <div class="cp-tip">
        <el-icon><InfoFilled /></el-icon>
        <span>修改成功后需要使用新密码重新登录</span>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { reactive, ref, computed } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { Lock, Key, CircleCheck, InfoFilled, Check, Back } from '@element-plus/icons-vue'
import { useAuthStore } from '@/stores/auth'
import { changePassword } from '@/api/auth'

const router = useRouter()
const auth = useAuthStore()
const formRef = ref<FormInstance>()
const loading = ref(false)
const form = reactive({ current: '', new: '', confirm: '' })

const rules: FormRules = {
  current: [{ required: true, message: '请输入当前密码', trigger: 'blur' }],
  new: [
    { required: true, message: '请输入新密码', trigger: 'blur' },
    { min: 6, message: '密码长度至少 6 位', trigger: 'blur' },
    {
      validator: (_r, value: string, cb) => {
        if (value && value === form.current) cb(new Error('新密码不能与当前密码相同'))
        else cb()
      },
      trigger: 'blur',
    },
  ],
  confirm: [
    { required: true, message: '请再次输入新密码', trigger: 'blur' },
    {
      validator: (_r, value: string, cb) => {
        if (value !== form.new) cb(new Error('两次输入的密码不一致'))
        else cb()
      },
      trigger: 'blur',
    },
  ],
}

// 密码强度：基于长度 + 字符多样性
function calcStrength(pw: string): { score: number; level: 'weak' | 'fair' | 'good' | 'strong'; label: string } {
  if (!pw) return { score: 0, level: 'weak', label: '' }
  let diversity = 0
  if (/[a-z]/.test(pw)) diversity++
  if (/[A-Z]/.test(pw)) diversity++
  if (/\d/.test(pw)) diversity++
  if (/[^\w\s]/.test(pw)) diversity++
  const lengthScore = pw.length >= 12 ? 3 : pw.length >= 8 ? 2 : 1
  const score = Math.min(4, lengthScore - 1 + diversity)
  if (score <= 1) return { score, level: 'weak', label: '弱' }
  if (score === 2) return { score, level: 'fair', label: '一般' }
  if (score === 3) return { score, level: 'good', label: '良好' }
  return { score, level: 'strong', label: '强' }
}
const strength = computed(() => calcStrength(form.new))
const strengthPercent = computed(() => `${(strength.value.score / 4) * 100}%`)
const strengthLevel = computed(() => strength.value.level)
const strengthLabel = computed(() => strength.value.label)

async function handleSubmit() {
  if (!formRef.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  loading.value = true
  try {
    // #11: 当前密码由服务端统一校验（不再用"再登录一次"在客户端代偿）；
    // 改密成功后服务端会轮换 api_token，旧 token 立即失效。
    await changePassword(form.current, form.new)
    ElMessage.success('密码修改成功，请使用新密码重新登录')
    auth.logout()
    router.push('/login')
  } catch (e) {
    // 401 = 当前密码错误（拦截器已对 /api/auth/password 的 401 透传，不触发全局登出）；
    // 其它错误由全局拦截器统一提示，无需重复。
    const status = (e as { response?: { status?: number } })?.response?.status
    if (status === 401) {
      ElMessage.warning('当前密码错误')
    } else {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
      ElMessage.error('密码修改失败，请稍后重试')
    }
  } finally {
    loading.value = false
  }
}

function goBack() {
  if (window.history.length > 1) router.back()
  else router.push('/overview')
}
</script>

<style scoped src="./ChangePassword.css"></style>