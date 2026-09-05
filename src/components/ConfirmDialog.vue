<script setup lang="ts">
/** 全局确认对话框（见 composables/confirm.ts 的说明）。 */
import { confirmState, settleConfirm } from "../composables/confirm";
</script>

<template>
  <Teleport to="body">
    <div v-if="confirmState.open" class="overlay" @click.self="settleConfirm(false)">
      <div class="dialog" role="alertdialog" :aria-label="confirmState.title">
        <h3>{{ confirmState.title }}</h3>
        <p class="msg">{{ confirmState.message }}</p>
        <div class="actions">
          <button class="btn cancel" @click="settleConfirm(false)">取消</button>
          <button
            class="btn ok"
            :class="{ danger: confirmState.danger }"
            @click="settleConfirm(true)"
          >
            {{ confirmState.confirmText }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}

.dialog {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 14px;
  padding: 20px 22px;
  width: min(420px, calc(100vw - 48px));
  box-shadow: 0 18px 50px rgba(0, 0, 0, 0.25);
}

.dialog h3 {
  margin: 0 0 8px;
  font-size: 15px;
}

.msg {
  margin: 0;
  font-size: 13px;
  color: var(--text-secondary);
  white-space: pre-line;
  line-height: 1.55;
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 18px;
}

.btn {
  padding: 7px 16px;
  border: none;
  border-radius: 8px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  font-family: inherit;
}

.btn.cancel {
  background: var(--surface-inset);
  color: var(--text);
}

.btn.ok {
  background: var(--accent);
  color: #fff;
}

.btn.ok.danger {
  background: var(--danger);
}
</style>
