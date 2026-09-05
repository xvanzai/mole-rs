<script setup lang="ts">
/**
 * 项目清理页（模块4）。
 *
 * 对标 `mo purge`：扫描搜索根下的项目构建产物（node_modules、target、
 * DerivedData 等），按项目分组；仅 7 天无活动的产物进入默认选择
 * （对标 classify_purge_activity，fail-closed）；删除走回收站（可恢复）。
 */
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "../composables/confirm";

interface PurgeArtifact {
  path: string;
  size_bytes: number;
  activity: string;
}
interface PurgeProject {
  project_dir: string;
  artifacts: PurgeArtifact[];
  total_size_bytes: number;
}
interface PurgeScanResult {
  projects: PurgeProject[];
  total_size_bytes: number;
  search_root_count: number;
}
interface DeleteOutcome {
  path: string;
  status: string;
  size_bytes: number;
  detail: string;
}
interface ExecuteResult {
  outcomes: DeleteOutcome[];
  deleted_count: number;
  freed_bytes: number;
  failed_count: number;
}

const scan = ref<PurgeScanResult | null>(null);
const loading = ref(false);
const error = ref("");
const executing = ref(false);
const result = ref<ExecuteResult | null>(null);
/** 是否已开始（不自动扫描，由用户显式触发）。 */
const started = ref(false);
/** 选中的产物路径（仅 old 活动）。 */
const selected = ref<Set<string>>(new Set());
const expanded = ref<string | null>(null);

async function load() {
  loading.value = true;
  error.value = "";
  try {
    scan.value = await invoke<PurgeScanResult>("purge_scan");
    selected.value = new Set(
      (scan.value?.projects ?? []).flatMap((p) =>
        p.artifacts.filter((a) => a.activity === "old").map((a) => a.path),
      ),
    );
    result.value = null;
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

/** 开始：项目产物只读扫描（对标 `mo purge --dry-run`）。 */
async function startScan() {
  started.value = true;
  await load();
}

async function execute() {
  if (!selected.value.size) return;
  const ok = await confirm(
    `将把 ${selected.value.size} 个构建产物移入废纸篓（可恢复）。\n` +
      "执行前会重新扫描并复核保护规则与活动状态。",
    { title: "执行项目清理", confirmText: "开始清理" },
  );
  if (!ok) return;
  executing.value = true;
  error.value = "";
  try {
    result.value = await invoke<ExecuteResult>("purge_execute", {
      selectedPaths: [...selected.value],
      dryRun: false,
    });
    await load();
  } catch (e) {
    error.value = String(e);
  } finally {
    executing.value = false;
  }
}

function toggle(path: string) {
  const next = new Set(selected.value);
  if (next.has(path)) {
    next.delete(path);
  } else {
    next.add(path);
  }
  selected.value = next;
}

const totalText = computed(() => {
  if (!scan.value) return "…";
  return mb(scan.value.total_size_bytes);
});

function mb(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(2)} MB`;
  return `${(bytes / 1073741824).toFixed(2)} GB`;
}

function shortPath(p: string): string {
  const parts = p.split("/").filter(Boolean);
  return parts.slice(-2).join("/");
}
</script>

<template>
  <section class="purge-page">
    <header class="head">
      <div>
        <h2>项目清理</h2>
        <p class="sub">
          扫描根 {{ scan?.search_root_count ?? "…" }} 个（默认路径 + 项目容器探针 +
          <code>~/.config/mole/purge_paths</code>）。仅展示 7 天无活动的产物。
        </p>
      </div>
      <div class="total">
        <span class="num">{{ totalText }}</span>
        <span class="label">可释放</span>
      </div>
      <button class="rescan" :disabled="loading || executing" @click="load">
        {{ loading ? "扫描中…" : "重新扫描" }}
      </button>
      <button
        class="execute"
        :disabled="loading || executing || !selected.size"
        @click="execute"
      >
        {{ executing ? "清理中…" : `清理选中（${selected.size}）` }}
      </button>
    </header>

    <div v-if="!started && !loading && !scan" class="idle">
      <p class="idle-icon">🗑️</p>
      <h3>项目清理</h3>
      <p class="sub">
        扫描搜索根下的项目构建产物（node_modules、target、DerivedData
        等，对标 <code>mo purge</code>）。仅 7 天无活动的产物进入默认选择。
      </p>
      <button class="start" :disabled="loading" @click="startScan">
        开始扫描
      </button>
    </div>

    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="result" class="result" :class="{ ok: result.failed_count === 0 }">
      已移入废纸篓 {{ result.deleted_count }} 项，释放 {{ mb(result.freed_bytes) }}
      <template v-if="result.failed_count">，失败 {{ result.failed_count }} 项</template>。
    </div>

    <div class="list">
      <article
        v-for="p in scan?.projects ?? []"
        :key="p.project_dir"
        class="project"
        @click="expanded = expanded === p.project_dir ? null : p.project_dir"
      >
        <div class="row">
          <span class="proj-name">{{ shortPath(p.project_dir) }}</span>
          <span class="proj-path mono" :title="p.project_dir">{{ p.project_dir }}</span>
          <span class="size">{{ mb(p.total_size_bytes) }}</span>
        </div>
        <div v-if="expanded === p.project_dir" class="artifacts">
          <label v-for="a in p.artifacts" :key="a.path" class="artifact" @click.stop>
            <input
              type="checkbox"
              :checked="selected.has(a.path)"
              @change="toggle(a.path)"
            />
            <code>{{ shortPath(a.path) }}</code>
            <span class="artifact-meta">
              {{ a.activity === "old" ? "可清理" : a.activity === "recent" ? "近期活动" : "不确定" }}
              · {{ mb(a.size_bytes) }}
            </span>
          </label>
        </div>
      </article>
      <p v-if="scan && !scan.projects.length && !loading" class="sub empty">
        没有发现项目构建产物。可在
        <code>~/.config/mole/purge_paths</code> 添加自定义扫描目录。
      </p>
    </div>
  </section>
</template>

<style scoped>
.purge-page {
  padding: 24px 28px;
  max-width: 920px;
  margin: 0 auto;
}

.head {
  display: flex;
  align-items: flex-start;
  gap: 16px;
  margin-bottom: 18px;
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

.total {
  margin-left: auto;
  text-align: right;
}

.total .num {
  display: block;
  font-size: 24px;
  font-weight: 700;
  color: var(--accent);
}

.total .label {
  font-size: 11px;
  color: var(--text-secondary);
}

.rescan {
  align-self: center;
  padding: 7px 14px;
  border: none;
  border-radius: 8px;
  background: var(--surface-inset);
  color: var(--text);
  font-size: 13px;
  cursor: pointer;
}

.execute {
  align-self: center;
  padding: 7px 14px;
  border: none;
  border-radius: 8px;
  background: var(--accent);
  color: #fff;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}

.rescan:disabled,
.execute:disabled {
  opacity: 0.5;
  cursor: default;
}

.error {
  color: var(--danger);
  font-size: 13px;
}

.result {
  margin-bottom: 12px;
  padding: 10px 14px;
  border-radius: 8px;
  background: var(--surface-inset);
  border: 1px solid var(--border);
  font-size: 12.5px;
}

.result.ok {
  border-color: var(--ok);
}

.project {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
  margin-bottom: 8px;
  padding: 12px 16px;
  cursor: pointer;
}

.row {
  display: flex;
  align-items: baseline;
  gap: 10px;
}

.proj-name {
  font-size: 13.5px;
  font-weight: 600;
}

.proj-path {
  flex: 1;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
}

.size {
  font-size: 13px;
  color: var(--accent);
  font-variant-numeric: tabular-nums;
}

.artifacts {
  margin-top: 10px;
  border-top: 1px solid var(--border);
  padding-top: 8px;
}

.artifact {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 4px 0;
  font-size: 12px;
  cursor: pointer;
}

.artifact input {
  accent-color: var(--accent);
}

.artifact code {
  font-family: "SF Mono", Menlo, monospace;
  color: var(--text);
}

.artifact-meta {
  margin-left: auto;
  color: var(--text-secondary);
}

.mono {
  font-family: "SF Mono", Menlo, monospace;
  font-size: 11px;
}

.empty {
  text-align: center;
  padding: 32px 0;
}

.idle {
  text-align: center;
  padding: 72px 24px;
  background: var(--surface);
  border: 1px dashed var(--border);
  border-radius: 14px;
}

.idle-icon {
  font-size: 34px;
  margin: 0 0 8px;
}

.idle h3 {
  margin: 0 0 6px;
  font-size: 16px;
}

.idle .sub {
  max-width: 460px;
  margin: 0 auto 18px;
}

.start {
  padding: 9px 26px;
  border: none;
  border-radius: 9px;
  background: var(--accent);
  color: #fff;
  font-size: 14px;
  font-weight: 600;
  cursor: pointer;
}

.start:disabled {
  opacity: 0.5;
  cursor: default;
}
</style>
