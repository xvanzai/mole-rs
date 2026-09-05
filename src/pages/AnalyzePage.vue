<script setup lang="ts">
/**
 * 磁盘分析页（模块5）。
 *
 * 对标 `mo analyze`：单层目录浏览 + 按需下钻（dirEntry/scanResult 模型）。
 * 容量语义与原实现一致（分配大小、扫描内硬链接去重、符号链接不计）。
 * 删除仅限当前层的直接子项，走回收站（可恢复）。
 */
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "../composables/confirm";

interface DirEntry {
  name: string;
  path: string;
  size: number;
  is_dir: boolean;
  last_access: number;
}
interface FileEntry {
  name: string;
  path: string;
  size: number;
}
interface ScanResult {
  path: string;
  entries: DirEntry[];
  large_files: FileEntry[];
  total_size: number;
  total_files: number;
  total_dirs: number;
  truncated: boolean;
}
interface ExecuteResult {
  outcomes: { path: string; status: string; size_bytes: number; detail: string }[];
  deleted_count: number;
  freed_bytes: number;
  failed_count: number;
}

const home = ref("");
const pathInput = ref("");
const scan = ref<ScanResult | null>(null);
const loading = ref(false);
const error = ref("");
const selected = ref<Set<string>>(new Set());
const executing = ref(false);
const result = ref<ExecuteResult | null>(null);
const showLarge = ref(false);

async function init() {
  try {
    home.value = await invoke<string>("get_home_dir");
    if (!pathInput.value) pathInput.value = home.value;
    await doScan(pathInput.value);
  } catch {
    await doScan("/");
  }
}
init();

async function doScan(target: string) {
  loading.value = true;
  error.value = "";
  result.value = null;
  try {
    scan.value = await invoke<ScanResult>("analyze_scan", { path: target });
    pathInput.value = scan.value.path;
    selected.value = new Set();
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

function openEntry(e: DirEntry) {
  if (e.is_dir) void doScan(e.path);
}

function toggle(p: string) {
  const next = new Set(selected.value);
  if (next.has(p)) next.delete(p);
  else next.add(p);
  selected.value = next;
}

async function removeSelected() {
  if (!selected.value.size || !scan.value) return;
  const ok = await confirm(
    `将把 ${selected.value.size} 项移入废纸篓（可恢复）。`,
    { title: "删除所选条目", confirmText: "移入废纸篓" },
  );
  if (!ok) return;
  executing.value = true;
  try {
    result.value = await invoke<ExecuteResult>("analyze_delete", {
      root: scan.value.path,
      selected: [...selected.value],
      dryRun: false,
    });
    await doScan(scan.value.path);
  } catch (e) {
    error.value = String(e);
  } finally {
    executing.value = false;
  }
}

const crumbs = computed(() => {
  const parts = (scan.value?.path ?? "").split("/").filter(Boolean);
  const list: { label: string; path: string }[] = [
    { label: "磁盘", path: "/" },
  ];
  let acc = "";
  for (const p of parts) {
    acc += `/${p}`;
    list.push({ label: p, path: acc });
  }
  return list;
});

const maxEntrySize = computed(() =>
  Math.max(1, ...(scan.value?.entries ?? []).map((e) => e.size)),
);

function mb(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(2)} MB`;
  return `${(bytes / 1073741824).toFixed(2)} GB`;
}
</script>

<template>
  <section class="analyze-page">
    <header class="head">
      <div class="crumbs">
        <template v-for="(c, i) in crumbs" :key="c.path">
          <span v-if="i" class="sep">/</span>
          <button class="crumb" @click="doScan(c.path)">{{ c.label }}</button>
        </template>
      </div>
      <div class="toolbar">
        <input
          v-model="pathInput"
          class="path-input mono"
          placeholder="/absolute/path（如 /Volumes 外置盘）"
          @keyup.enter="doScan(pathInput)"
        />
        <button class="rescan" :disabled="loading" @click="doScan(pathInput)">
          {{ loading ? "扫描中…" : "扫描" }}
        </button>
        <button
          class="execute"
          :disabled="executing || !selected.size"
          @click="removeSelected"
        >
          {{ executing ? "处理中…" : `移入废纸篓（${selected.size}）` }}
        </button>
      </div>
    </header>

    <p v-if="error" class="error">{{ error }}</p>
    <p v-if="scan?.truncated" class="warn">
      扫描超时（30s 预算），结果为部分值，仅供浏览参考。
    </p>

    <div v-if="scan" class="stats sub">
      共 {{ mb(scan.total_size) }} · {{ scan.total_files }} 文件 ·
      {{ scan.total_dirs }} 目录
      <button class="link" @click="showLarge = !showLarge">
        {{ showLarge ? "隐藏" : "显示" }}大文件 Top{{ scan.large_files.length }}
      </button>
    </div>

    <div v-if="showLarge && scan" class="large">
      <div v-for="f in scan.large_files" :key="f.path" class="large-row">
        <code>{{ f.path }}</code>
        <span class="size">{{ mb(f.size) }}</span>
      </div>
    </div>

    <div class="list">
      <div
        v-for="e in scan?.entries ?? []"
        :key="e.path"
        class="entry"
        :class="{ dir: e.is_dir }"
        @click="openEntry(e)"
      >
        <label @click.stop>
          <input
            type="checkbox"
            :checked="selected.has(e.path)"
            @change="toggle(e.path)"
          />
        </label>
        <span class="icon">{{ e.is_dir ? "📁" : "📄" }}</span>
        <span class="name">{{ e.name }}</span>
        <div class="bar">
          <div class="bar-fill" :style="{ width: `${(e.size / maxEntrySize) * 100}%` }" />
        </div>
        <span class="size">{{ mb(e.size) }}</span>
        <span v-if="e.is_dir" class="hint">›</span>
      </div>
      <p v-if="scan && !scan.entries.length && !loading" class="sub empty">
        此目录为空（或全部内容为符号链接）。
      </p>
    </div>
  </section>
</template>

<style scoped>
.analyze-page {
  padding: 24px 28px;
  max-width: 980px;
  margin: 0 auto;
}

.head {
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin-bottom: 14px;
}

.crumbs {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 2px;
  font-size: 13px;
}

.crumb {
  border: none;
  background: transparent;
  color: var(--accent);
  cursor: pointer;
  padding: 2px 4px;
  font-size: 13px;
}

.crumb:hover {
  text-decoration: underline;
}

.sep {
  color: var(--text-secondary);
}

.toolbar {
  display: flex;
  gap: 10px;
}

.path-input {
  flex: 1;
  padding: 7px 10px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
  color: var(--text);
  font-size: 12px;
}

.rescan {
  padding: 7px 14px;
  border: none;
  border-radius: 8px;
  background: var(--surface-inset);
  color: var(--text);
  font-size: 13px;
  cursor: pointer;
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

.execute:disabled,
.rescan:disabled {
  opacity: 0.5;
  cursor: default;
}

.stats {
  margin-bottom: 10px;
}

.link {
  border: none;
  background: transparent;
  color: var(--accent);
  cursor: pointer;
  font-size: 12px;
  padding: 0 0 0 8px;
}

.large {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 10px 14px;
  margin-bottom: 12px;
  max-height: 240px;
  overflow: auto;
}

.large-row {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  padding: 3px 0;
  font-size: 11.5px;
}

.large-row code {
  font-family: "SF Mono", Menlo, monospace;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.entry {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 12px;
  margin-bottom: 4px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  cursor: pointer;
}

.entry:hover {
  border-color: var(--accent);
}

.entry input {
  accent-color: var(--accent);
}

.icon {
  font-size: 14px;
}

.name {
  flex: 1;
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.bar {
  flex: 0 0 160px;
  height: 6px;
  background: var(--surface-inset);
  border-radius: 3px;
  overflow: hidden;
}

.bar-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 3px;
}

.size {
  flex: 0 0 84px;
  text-align: right;
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  color: var(--text-secondary);
}

.hint {
  color: var(--text-secondary);
}

.mono {
  font-family: "SF Mono", Menlo, monospace;
}

.sub {
  color: var(--text-secondary);
  font-size: 12px;
}

.error {
  color: var(--danger);
  font-size: 13px;
}

.warn {
  color: var(--warning);
  font-size: 12.5px;
}

.empty {
  text-align: center;
  padding: 32px 0;
}
</style>
