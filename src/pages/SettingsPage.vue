<script setup lang="ts">
/**
 * 设置页（模块8b）。
 *
 * 白名单（对标 `mo clean --whitelist` / `mo optimize --whitelist`）：
 * 受保护路径不会出现在任何清理/删除列表；条目校验与服务端一致
 * （系统路径与 // 拒绝、~ 展开）。
 * purge_paths（对标 `mo purge --paths`）：额外项目扫描根。
 */
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

interface WhitelistConfig {
  path: string;
  entries: string[];
}
interface PurgePathsConfig {
  path: string;
  entries: string[];
}

const whitelist = ref<WhitelistConfig | null>(null);
const purgePaths = ref<PurgePathsConfig | null>(null);
const error = ref("");
const saved = ref("");

const newWhitelistEntry = ref("");
const newPurgePath = ref("");

async function load() {
  try {
    whitelist.value = await invoke<WhitelistConfig>("get_whitelist");
    purgePaths.value = await invoke<PurgePathsConfig>("get_purge_paths");
  } catch (e) {
    error.value = String(e);
  }
}
onMounted(load);

async function saveWhitelist() {
  if (!whitelist.value) return;
  error.value = "";
  saved.value = "";
  try {
    await invoke("set_whitelist", { lines: whitelist.value.entries });
    saved.value = "白名单已保存";
  } catch (e) {
    error.value = String(e);
  }
}

async function savePurgePaths() {
  if (!purgePaths.value) return;
  error.value = "";
  saved.value = "";
  try {
    await invoke("set_purge_paths", { lines: purgePaths.value.entries });
    saved.value = "项目扫描路径已保存";
  } catch (e) {
    error.value = String(e);
  }
}

function addWhitelistEntry() {
  const v = newWhitelistEntry.value.trim();
  if (!v || !whitelist.value) return;
  whitelist.value.entries.push(v);
  newWhitelistEntry.value = "";
}

function removeEntry(lines: string[], idx: number) {
  lines.splice(idx, 1);
}

function addPurgePath() {
  const v = newPurgePath.value.trim();
  if (!v || !purgePaths.value) return;
  purgePaths.value.entries.push(v);
  newPurgePath.value = "";
}
</script>

<template>
  <section class="settings-page">
    <header class="head">
      <div>
        <h2>设置</h2>
        <p class="sub">
          白名单中的路径永远不会出现在清理/卸载候选列表（含其子路径）。
          修改即时写入 <code>~/.config/mole/</code>。
        </p>
      </div>
    </header>

    <p v-if="error" class="error">{{ error }}</p>
    <p v-if="saved" class="saved">{{ saved }}</p>

    <article class="card">
      <h3>白名单保护</h3>
      <p class="sub file">文件：{{ whitelist?.path }}</p>
      <div v-for="(entry, i) in whitelist?.entries ?? []" :key="i" class="entry-row">
        <code>{{ entry }}</code>
        <button class="remove" @click="removeEntry(whitelist!.entries, i)">删除</button>
      </div>
      <div v-if="!whitelist?.entries.length" class="sub empty">白名单为空（无额外保护）。</div>
      <div class="add-row">
        <input
          v-model="newWhitelistEntry"
          class="add-input mono"
          placeholder="~/Library/Caches/某应用"
          @keyup.enter="addWhitelistEntry"
        />
        <button class="add" @click="addWhitelistEntry">添加</button>
        <button class="save" @click="saveWhitelist">保存</button>
      </div>
    </article>

    <article class="card">
      <h3>项目扫描路径（purge）</h3>
      <p class="sub file">文件：{{ purgePaths?.path }}</p>
      <div v-for="(entry, i) in purgePaths?.entries ?? []" :key="i" class="entry-row">
        <code>{{ entry }}</code>
        <button class="remove" @click="removeEntry(purgePaths!.entries, i)">删除</button>
      </div>
      <div v-if="!purgePaths?.entries.length" class="sub empty">
        未配置（使用默认搜索路径 + HOME 一级容器探针）。
      </div>
      <div class="add-row">
        <input
          v-model="newPurgePath"
          class="add-input mono"
          placeholder="/Volumes/数据盘/Projects"
          @keyup.enter="addPurgePath"
        />
        <button class="add" @click="addPurgePath">添加</button>
        <button class="save" @click="savePurgePaths">保存</button>
      </div>
    </article>
  </section>
</template>

<style scoped>
.settings-page {
  padding: 24px 28px;
  max-width: 760px;
  margin: 0 auto;
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

.card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 16px 18px;
  margin-top: 16px;
}

.card h3 {
  margin: 0 0 4px;
  font-size: 14px;
}

.file {
  font-size: 11px;
  margin-bottom: 10px;
}

.entry-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 5px 0;
  border-bottom: 1px solid var(--border);
}

.entry-row code {
  flex: 1;
  font-family: "SF Mono", Menlo, monospace;
  font-size: 12px;
}

.remove {
  border: none;
  background: transparent;
  color: var(--danger);
  cursor: pointer;
  font-size: 12px;
}

.add-row {
  display: flex;
  gap: 8px;
  margin-top: 12px;
}

.add-input {
  flex: 1;
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg);
  color: var(--text);
  font-size: 12px;
}

.add,
.save {
  padding: 6px 14px;
  border: none;
  border-radius: 8px;
  cursor: pointer;
  font-size: 12.5px;
}

.add {
  background: var(--surface-inset);
  color: var(--text);
}

.save {
  background: var(--accent);
  color: #fff;
  font-weight: 600;
}

.mono {
  font-family: "SF Mono", Menlo, monospace;
}

.error {
  color: var(--danger);
  font-size: 13px;
}

.saved {
  color: var(--ok);
  font-size: 13px;
}

.empty {
  padding: 8px 0;
}
</style>
