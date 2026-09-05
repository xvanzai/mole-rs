<script setup lang="ts">
/**
 * 优化维护页（模块7第一片）。
 *
 * 对标 `mo optimize`：21 项任务目录 1:1；已移植任务可执行（dry-run
 * 预览或真实执行），未移植任务标注"待迁移"（后端返回 unavailable）。
 */
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

interface OptimizeTask {
  action: string;
  health_name: string;
  description: string;
  implemented: boolean;
}
interface TaskResult {
  action: string;
  outcome: string;
  detail: string;
}
interface OptimizeResult {
  results: TaskResult[];
  applied: number;
  unchanged: number;
  skipped: number;
  unavailable: number;
  attention: number;
  failed: number;
}

const tasks = ref<OptimizeTask[]>([]);
const selected = ref<Set<string>>(new Set());
const loading = ref(false);
const executing = ref(false);
const error = ref("");
const result = ref<OptimizeResult | null>(null);
const dryRun = ref(true);

async function load() {
  loading.value = true;
  try {
    tasks.value = await invoke<OptimizeTask[]>("optimize_tasks");
    // 默认勾选已实现任务。
    selected.value = new Set(tasks.value.filter((t) => t.implemented).map((t) => t.action));
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

async function execute() {
  if (!selected.value.size) return;
  executing.value = true;
  error.value = "";
  try {
    result.value = await invoke<OptimizeResult>("optimize_execute", {
      selected: [...selected.value],
      dryRun: dryRun.value,
    });
  } catch (e) {
    error.value = String(e);
  } finally {
    executing.value = false;
  }
}

function toggle(action: string) {
  const next = new Set(selected.value);
  if (next.has(action)) next.delete(action);
  else next.add(action);
  selected.value = next;
}

const outcomeBadge: Record<string, string> = {
  applied: "✅ 已应用",
  unchanged: "🔄 无变化",
  skipped: "⏭ 跳过",
  unavailable: "🚧 待迁移",
  attention: "⚠️ 需注意",
  failed: "❌ 失败",
};

</script>

<template>
  <section class="optimize-page">
    <header class="head">
      <div>
        <h2>优化维护</h2>
        <p class="sub">
          {{ tasks.length }} 项系统维护任务（对标 <code>mo optimize</code> 目录）。
          有界执行、逐项可解释；处理器按计划逐个对标移植中。
        </p>
      </div>
      <label class="dry-run">
        <input v-model="dryRun" type="checkbox" />
        dry-run 预览
      </label>
      <button
        class="execute"
        :disabled="executing || !selected.size"
        @click="execute"
      >
        {{ executing ? "执行中…" : `执行选中（${selected.size}）` }}
      </button>
    </header>

    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="result" class="result">
      <p class="summary">
        已应用 {{ result.applied }} · 无变化 {{ result.unchanged }} · 跳过
        {{ result.skipped }} · 待迁移 {{ result.unavailable }} · 失败
        {{ result.failed }}
      </p>
      <div v-for="r in result.results" :key="r.action" class="result-row">
        <span class="badge">{{ outcomeBadge[r.outcome] ?? r.outcome }}</span>
        <span>{{ r.action }}</span>
        <span class="detail">{{ r.detail }}</span>
      </div>
    </div>

    <div class="list">
      <div
        v-for="t in tasks"
        :key="t.action"
        class="task"
        :class="{ disabled: !t.implemented }"
      >
        <input
          type="checkbox"
          :disabled="!t.implemented"
          :checked="selected.has(t.action)"
          @change="toggle(t.action)"
        />
        <div class="task-body">
          <span class="task-name">{{ t.health_name }}</span>
          <span class="task-desc">{{ t.description }}</span>
        </div>
        <span v-if="!t.implemented" class="pending">待迁移</span>
      </div>
    </div>
  </section>
</template>

<style scoped>
.optimize-page {
  padding: 24px 28px;
  max-width: 880px;
  margin: 0 auto;
}

.head {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 16px;
}

.head h2 {
  margin: 0;
  font-size: 18px;
}

.sub {
  margin: 4px 0 0;
  color: var(--text-secondary);
  font-size: 12.5px;
}

.dry-run {
  margin-left: auto;
  font-size: 12.5px;
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
}

.dry-run input {
  accent-color: var(--accent);
}

.execute {
  padding: 7px 14px;
  border: none;
  border-radius: 8px;
  background: var(--accent);
  color: #fff;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}

.execute:disabled {
  opacity: 0.5;
  cursor: default;
}

.error {
  color: var(--danger);
  font-size: 13px;
}

.result {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 12px 16px;
  margin-bottom: 14px;
}

.summary {
  margin: 0 0 8px;
  font-size: 12.5px;
  font-weight: 600;
}

.result-row {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 12px;
  padding: 3px 0;
}

.badge {
  flex: 0 0 84px;
}

.detail {
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.task {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 9px 14px;
  margin-bottom: 4px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
}

.task.disabled {
  opacity: 0.55;
}

.task input {
  accent-color: var(--accent);
  margin-top: 3px;
}

.task-body {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.task-name {
  font-size: 13px;
  font-weight: 500;
}

.task-desc {
  font-size: 12px;
  color: var(--text-secondary);
}

.pending {
  font-size: 10.5px;
  padding: 1px 8px;
  border-radius: 8px;
  background: var(--surface-inset);
  color: var(--text-secondary);
  white-space: nowrap;
  margin-top: 3px;
}
</style>
