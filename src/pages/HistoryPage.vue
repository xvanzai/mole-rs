<script setup lang="ts">
/**
 * 历史记录页（模块8第一片）。
 *
 * 对标 `mo history --json`：operations.log 的会话聚合 + deletions.log
 * 取证记录。数据由 clean/purge/analyze 各模块的统一日志产生。
 */
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

interface HistorySession {
  command: string;
  started_at: string;
  ended_at: string | null;
  items: number | null;
  size_human: string | null;
  removed: number;
  trashed: number;
  skipped: number;
  failed: number;
  rebuilt: number;
  other: number;
  operations: number;
}
interface DeletionRecord {
  timestamp: string;
  mode: string;
  size_kb: string;
  status: string;
  path: string;
}
interface HistoryData {
  sessions: HistorySession[];
  deletions: DeletionRecord[];
}

const data = ref<HistoryData | null>(null);
const loading = ref(true);
const error = ref("");

async function load() {
  loading.value = true;
  error.value = "";
  try {
    data.value = await invoke<HistoryData>("history_list", { limit: 50 });
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

function summary(s: HistorySession): string {
  const parts: string[] = [];
  if (s.removed) parts.push(`删除 ${s.removed}`);
  if (s.trashed) parts.push(`回收站 ${s.trashed}`);
  if (s.skipped) parts.push(`跳过 ${s.skipped}`);
  if (s.failed) parts.push(`失败 ${s.failed}`);
  if (s.rebuilt) parts.push(`重建 ${s.rebuilt}`);
  if (s.other) parts.push(`其他 ${s.other}`);
  return parts.length ? parts.join(" · ") : "无文件操作";
}

const commandIcon: Record<string, string> = {
  clean: "🧹",
  purge: "🗑️",
  analyze: "🔍",
  uninstall: "📦",
};
</script>

<template>
  <section class="history-page">
    <header class="head">
      <div>
        <h2>历史记录</h2>
        <p class="sub">
          来自 <code>~/Library/Logs/mole/operations.log</code> 与
          <code>deletions.log</code>（对标 <code>mo history</code>）。
        </p>
      </div>
      <button class="rescan" :disabled="loading" @click="load">
        {{ loading ? "读取中…" : "刷新" }}
      </button>
    </header>

    <p v-if="error" class="error">{{ error }}</p>

    <template v-if="data">
      <h3 class="section">最近会话</h3>
      <article v-for="(s, i) in data.sessions" :key="i" class="session">
        <div class="row">
          <span class="cmd">{{ commandIcon[s.command] ?? "⚙️" }} {{ s.command }}</span>
          <span class="time">{{ s.started_at }}<template v-if="s.ended_at"> → {{ s.ended_at }}</template></span>
        </div>
        <p class="detail">
          {{ summary(s) }}
          <template v-if="s.items !== null"> · {{ s.items }} 项<template v-if="s.size_human">，{{ s.size_human }}</template></template>
        </p>
      </article>
      <p v-if="!data.sessions.length && !loading" class="sub empty">
        暂无会话记录——先去清理或分析一次吧。
      </p>

      <h3 class="section">删除取证</h3>
      <table v-if="data.deletions.length" class="deletions">
        <thead>
          <tr><th>时间</th><th>方式</th><th>状态</th><th>大小</th><th>路径</th></tr>
        </thead>
        <tbody>
          <tr v-for="(d, i) in data.deletions" :key="i">
            <td class="mono">{{ d.timestamp }}</td>
            <td>{{ d.mode }}</td>
            <td>{{ d.status }}</td>
            <td>{{ d.size_kb }} KB</td>
            <td class="mono path">{{ d.path }}</td>
          </tr>
        </tbody>
      </table>
      <p v-else class="sub empty">暂无删除记录。</p>
    </template>
  </section>
</template>

<style scoped>
.history-page {
  padding: 24px 28px;
  max-width: 920px;
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

.rescan {
  margin-left: auto;
  padding: 7px 14px;
  border: none;
  border-radius: 8px;
  background: var(--surface-inset);
  color: var(--text);
  font-size: 13px;
  cursor: pointer;
}

.rescan:disabled {
  opacity: 0.5;
}

.error {
  color: var(--danger);
  font-size: 13px;
}

.section {
  font-size: 13px;
  color: var(--text-secondary);
  margin: 18px 0 8px;
}

.session {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
  margin-bottom: 8px;
  padding: 10px 16px;
}

.row {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
}

.cmd {
  font-size: 13.5px;
  font-weight: 600;
  text-transform: capitalize;
}

.time {
  font-size: 11.5px;
  color: var(--text-secondary);
  font-family: "SF Mono", Menlo, monospace;
}

.detail {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--text-secondary);
}

.deletions {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}

.deletions th {
  text-align: left;
  color: var(--text-secondary);
  font-weight: 500;
  padding: 4px 10px 6px 0;
  border-bottom: 1px solid var(--border);
}

.deletions td {
  padding: 5px 10px 5px 0;
  border-bottom: 1px solid var(--border);
}

.path {
  max-width: 360px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mono {
  font-family: "SF Mono", Menlo, monospace;
  font-size: 11px;
}

.empty {
  text-align: center;
  padding: 20px 0;
}
</style>
