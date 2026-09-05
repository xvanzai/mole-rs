<script setup lang="ts">
/**
 * 深度清理页（模块3a：只读预览）。
 *
 * 对标 `MOLE_DRY_RUN=1 ./mole clean` 的 dry-run 输出；删除执行将在
 * 模块3b（完整 should_protect_path 保护层 + Trash 路由 + 操作日志）
 * 落地后开放——安全契约：不可预览的删除不存在。
 */
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

interface CleanItem {
  path: string;
  size_bytes: number;
  skip_reason: string;
}
interface CleanGroup {
  description: string;
  family: string;
  items: CleanItem[];
  total_size_bytes: number;
  skipped_count: number;
}
interface CleanPreview {
  groups: CleanGroup[];
  total_size_bytes: number;
  whitelist_source: string;
}

const preview = ref<CleanPreview | null>(null);
const loading = ref(true);
const error = ref("");
/** 选中的组描述（对标勾选清理项）；默认全选有可释放空间的组。 */
const selected = ref<Set<string>>(new Set());
const executing = ref(false);
const result = ref<ExecuteResult | null>(null);

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

async function load() {
  loading.value = true;
  error.value = "";
  try {
    preview.value = await invoke<CleanPreview>("clean_preview");
    // 默认全选有可释放空间的组。
    selected.value = new Set(
      (preview.value?.groups ?? [])
        .filter((g) => g.total_size_bytes > 0)
        .map((g) => g.description),
    );
    result.value = null;
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

/** 执行清理：确认对话框 → 后端重扫 + sink 复检 + Trash 删除。 */
async function execute() {
  if (!selected.value.size) return;
  const ok = window.confirm(
    `将把 ${selected.value.size} 组缓存移入废纸篓（可恢复）。\n` +
      "执行前会重新扫描并在删除时再次校验保护与白名单。继续？",
  );
  if (!ok) return;
  executing.value = true;
  error.value = "";
  try {
    result.value = await invoke<ExecuteResult>("clean_execute", {
      selectedGroups: [...selected.value],
      dryRun: false,
    });
    await load(); // 执行后刷新扫描结果
  } catch (e) {
    error.value = String(e);
  } finally {
    executing.value = false;
  }
}

function toggleGroup(desc: string) {
  const next = new Set(selected.value);
  if (next.has(desc)) {
    next.delete(desc);
  } else {
    next.add(desc);
  }
  selected.value = next;
}

const totalText = computed(() => {
  if (!preview.value) return "…";
  return `${(preview.value.total_size_bytes / 1048576).toFixed(2)} MB`;
});

const activeGroups = computed(() =>
  (preview.value?.groups ?? []).filter((g) => g.total_size_bytes > 0).sort((a, b) => b.total_size_bytes - a.total_size_bytes),
);
const expanded = ref<string | null>(null);

function mb(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(2)} MB`;
  return `${(bytes / 1073741824).toFixed(2)} GB`;
}
</script>

<template>
  <section class="clean-page">
    <header class="head">
      <div>
        <h2>深度清理</h2>
        <p class="sub">
          Apple 系统缓存族（对标 <code>clean_app_caches</code>）。删除统一走
          回收站（可恢复），受保护与白名单路径双重拦截。
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

    <div v-if="result" class="result" :class="{ ok: result.failed_count === 0 }">
      已移入废纸篓 {{ result.deleted_count }} 项，释放
      {{ mb(result.freed_bytes) }}
      <template v-if="result.failed_count">，失败 {{ result.failed_count }} 项</template>。
      日志见 <code>~/Library/Logs/mole/operations.log</code>。
    </div>

    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="preview" class="list">
      <p class="sub whitelist-note">
        白名单来源：{{ preview.whitelist_source }} · 受保护与白名单路径已自动跳过
      </p>
      <article
        v-for="g in activeGroups"
        :key="g.description"
        class="group"
        @click="expanded = expanded === g.description ? null : g.description"
      >
        <div class="row">
          <label class="check" @click.stop>
            <input
              type="checkbox"
              :checked="selected.has(g.description)"
              @change="toggleGroup(g.description)"
            />
          </label>
          <span class="desc">{{ g.description }}<span class="fam">{{ g.family }}</span></span>
          <span class="size">{{ mb(g.total_size_bytes) }}</span>
        </div>
        <div v-if="expanded === g.description" class="items">
          <div v-for="item in g.items" :key="item.path" class="item">
            <code>{{ item.path }}</code>
            <span class="item-size">
              {{ item.skip_reason ? `已跳过（${item.skip_reason}）` : mb(item.size_bytes) }}
            </span>
          </div>
        </div>
      </article>
      <p v-if="!activeGroups.length && !loading" class="sub empty">
        没有发现可清理的缓存——你的系统很干净。
      </p>
    </div>
  </section>
</template>

<style scoped>
.clean-page {
  padding: 24px 28px;
  max-width: 860px;
  margin: 0 auto;
}

.head {
  display: flex;
  align-items: flex-start;
  gap: 20px;
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

.rescan:disabled,
.execute:disabled {
  opacity: 0.5;
  cursor: default;
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

.check {
  display: flex;
  align-items: center;
  cursor: pointer;
}

.check input {
  accent-color: var(--accent);
}

.error {
  color: var(--danger);
  font-size: 13px;
}

.whitelist-note {
  margin-bottom: 12px;
}

.group {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
  margin-bottom: 8px;
  padding: 12px 16px;
  cursor: pointer;
}

.row {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.desc {
  font-size: 13.5px;
  font-weight: 500;
}

.fam {
  margin-left: 8px;
  font-size: 10.5px;
  color: var(--text-secondary);
  background: var(--surface-inset);
  border-radius: 4px;
  padding: 1px 6px;
}

.size {
  font-size: 13px;
  color: var(--accent);
  font-variant-numeric: tabular-nums;
}

.items {
  margin-top: 10px;
  border-top: 1px solid var(--border);
  padding-top: 8px;
}

.item {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  padding: 3px 0;
  font-size: 11.5px;
}

.item code {
  font-family: "SF Mono", Menlo, monospace;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.item-size {
  flex: none;
  color: var(--text-secondary);
}

.empty {
  text-align: center;
  padding: 32px 0;
}
</style>
