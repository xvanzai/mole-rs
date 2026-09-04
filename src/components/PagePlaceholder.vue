<script setup lang="ts">
defineProps<{
  /** 模块名 */
  label: string;
  /** 对标的 Mole 原代码位置 */
  origin: string;
  /** 迁移状态 */
  status: string;
  /** 一句话功能说明 */
  description: string;
}>();
</script>

<template>
  <section class="placeholder">
    <div class="card">
      <h2>{{ label }}</h2>
      <p class="desc">{{ description }}</p>
      <dl>
        <div class="row">
          <dt>对标原代码</dt>
          <dd><code>{{ origin }}</code></dd>
        </div>
        <div class="row">
          <dt>迁移状态</dt>
          <dd>{{ status }}</dd>
        </div>
      </dl>
      <p class="hint">
        本模块尚未迁移，迁移顺序与安全契约见
        <code>docs/migration/PLAN.md</code>。
      </p>
    </div>
  </section>
</template>

<style scoped>
.placeholder {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 32px;
}

.card {
  max-width: 460px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 28px 32px;
}

h2 {
  margin: 0 0 8px;
  font-size: 18px;
}

.desc {
  margin: 0 0 20px;
  color: var(--text-secondary);
}

dl {
  margin: 0 0 16px;
}

.row {
  display: flex;
  gap: 12px;
  padding: 6px 0;
  border-bottom: 1px solid var(--border);
}

.row:last-child {
  border-bottom: none;
}

dt {
  flex: 0 0 72px;
  color: var(--text-secondary);
  font-size: 13px;
}

dd {
  margin: 0;
  font-size: 13px;
}

.hint {
  margin: 0;
  font-size: 12px;
  color: var(--text-secondary);
}

code {
  font-family: "SF Mono", Menlo, monospace;
  font-size: 12px;
  background: var(--surface-inset);
  padding: 1px 5px;
  border-radius: 4px;
}
</style>
