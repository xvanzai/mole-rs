<script setup lang="ts">
/**
 * 应用卸载页（模块6第一片：只读清单）。
 *
 * 对标 `mo uninstall` 列表阶段：搜索目录 + Info.plist bundle ID
 * （含 iOS Wrapper 回退）+ 卸载模式保护分级（系统关键 🛡 / Apple 可卸载）。
 * 应用本体删除与残留查找（find_app_files）在下一子片落地。
 */
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

interface AppInfo {
  path: string;
  name: string;
  bundle_id: string;
  version: string;
  size_bytes: number;
  protected: boolean;
  uninstallable: boolean;
  background_only: boolean;
  in_search_root: boolean;
}

const apps = ref<AppInfo[]>([]);
const loading = ref(false);
const error = ref("");
const query = ref("");
/** 是否已开始（不自动扫描，由用户显式触发）。 */
const started = ref(false);

async function load() {
  loading.value = true;
  error.value = "";
  try {
    apps.value = await invoke<AppInfo[]>("uninstall_list_apps");
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

/** 开始：应用清单扫描（并行测径，仅只读）。 */
async function startScan() {
  started.value = true;
  await load();
}

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return apps.value;
  return apps.value.filter(
    (a) =>
      a.name.toLowerCase().includes(q) || a.bundle_id.toLowerCase().includes(q),
  );
});

const totalSize = computed(() =>
  apps.value.reduce((acc, a) => acc + a.size_bytes, 0),
);

function mb(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(1)} MB`;
  return `${(bytes / 1073741824).toFixed(2)} GB`;
}
</script>

<template>
  <section class="uninstall-page">
    <header class="head">
      <div>
        <h2>应用卸载</h2>
        <p class="sub" v-if="started || apps.length">
          {{ apps.length }} 个应用 · 共 {{ mb(totalSize) }}。删除与残留清理将在
          下一子片开放（需逐行移植 find_app_files 安全汇）。
        </p>
        <p class="sub" v-else>应用清单与保护分级（只读，对标 <code>mo uninstall</code>）。</p>
      </div>
      <input v-model="query" class="search" placeholder="搜索应用或 Bundle ID…" />
      <button class="rescan" :disabled="loading" @click="load">
        {{ loading ? "扫描中…" : "重新扫描" }}
      </button>
    </header>

    <div v-if="!started && !loading && !apps.length" class="idle">
      <p class="idle-icon">📦</p>
      <h3>应用卸载</h3>
      <p class="sub">
        扫描 /Applications、~/Applications、输入法目录与外置卷
        （对标 <code>mo uninstall</code> 清单阶段），并标记系统保护应用。
      </p>
      <button class="start" :disabled="loading" @click="startScan">
        开始扫描
      </button>
    </div>

    <p v-if="error" class="error">{{ error }}</p>

    <div class="list">
      <div v-for="a in filtered" :key="a.path" class="app" :title="a.path">
        <span class="name">{{ a.name }}</span>
        <span class="ver">{{ a.version || "—" }}</span>
        <code class="bundle">{{ a.bundle_id }}</code>
        <span class="badges">
          <span v-if="a.protected" class="badge shield">🛡 系统保护</span>
          <span v-else-if="a.uninstallable" class="badge store">Apple 可卸载</span>
          <span v-if="a.background_only" class="badge">后台</span>
        </span>
        <span class="size">{{ mb(a.size_bytes) }}</span>
      </div>
      <p v-if="started && !filtered.length && !loading" class="sub empty">没有匹配的应用。</p>
    </div>
  </section>
</template>

<style scoped>
.uninstall-page {
  padding: 24px 28px;
  max-width: 980px;
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

.search {
  margin-left: auto;
  padding: 7px 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
  color: var(--text);
  font-size: 13px;
  width: 220px;
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

.rescan:disabled {
  opacity: 0.5;
}

.error {
  color: var(--danger);
  font-size: 13px;
}

.app {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 14px;
  margin-bottom: 4px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
}

.name {
  flex: 0 0 180px;
  font-size: 13px;
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ver {
  flex: 0 0 70px;
  font-size: 12px;
  color: var(--text-secondary);
}

.bundle {
  flex: 1;
  font-family: "SF Mono", Menlo, monospace;
  font-size: 11px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.badges {
  display: flex;
  gap: 6px;
}

.badge {
  font-size: 10.5px;
  padding: 1px 8px;
  border-radius: 8px;
  background: var(--surface-inset);
  color: var(--text-secondary);
  white-space: nowrap;
}

.badge.shield {
  background: rgba(255, 159, 10, 0.15);
  color: var(--warning);
}

.badge.store {
  background: rgba(48, 209, 88, 0.15);
  color: var(--ok);
}

.size {
  flex: 0 0 84px;
  text-align: right;
  font-size: 12px;
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
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
  margin-bottom: 14px;
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
