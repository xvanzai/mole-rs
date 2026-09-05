<script setup lang="ts">
import { computed, ref } from "vue";
import PagePlaceholder from "./components/PagePlaceholder.vue";
import StatusPage from "./pages/StatusPage.vue";
import CleanPage from "./pages/CleanPage.vue";
import PurgePage from "./pages/PurgePage.vue";

/**
 * 应用外壳：侧边导航 + 模块页面切换。
 *
 * 模块划分对标 Mole CLI 子命令（见 docs/migration/PLAN.md §3），
 * 各页面按迁移顺序逐个实现，先以占位页呈现。
 */

interface ModuleDef {
  id: string;
  label: string;
  icon: string;
  /** 对标的 Mole 原代码位置 */
  origin: string;
  /** 迁移状态文案 */
  status: string;
  description: string;
  /** 是否已实现（已实现模块将替换占位组件） */
  implemented?: boolean;
}

const modules: ModuleDef[] = [
  {
    id: "status",
    label: "系统监控",
    icon: "📊",
    origin: "Mole cmd/status/*.go",
    status: "模块 2 · 已完成",
    description: "实时查看 CPU、内存、磁盘、网络、电池与健康状态（只读）。",
    implemented: true,
  },
  {
    id: "clean",
    label: "深度清理",
    icon: "🧹",
    origin: "Mole bin/clean.sh + lib/clean/*",
    status: "模块 3a · 预览版",
    description: "扫描系统、开发工具与浏览器缓存，预览后安全清理。",
    implemented: true,
  },
  {
    id: "analyze",
    label: "磁盘分析",
    icon: "🔍",
    origin: "Mole cmd/analyze/*.go",
    status: "模块 5 · 待迁移",
    description: "可视化磁盘占用分布，定位大文件与目录。",
  },
  {
    id: "uninstall",
    label: "应用卸载",
    icon: "📦",
    origin: "Mole bin/uninstall.sh + lib/uninstall/*",
    status: "模块 6 · 待迁移",
    description: "卸载应用及其残留（启动项、偏好设置、隐藏文件）。",
  },
  {
    id: "optimize",
    label: "优化维护",
    icon: "⚡️",
    origin: "Mole bin/optimize.sh + lib/optimize/*",
    status: "模块 7 · 待迁移",
    description: "刷新系统缓存与服务等有界维护任务。",
  },
  {
    id: "purge",
    label: "项目清理",
    icon: "🗑️",
    origin: "Mole bin/purge.sh + lib/clean/project.sh",
    status: "模块 4 · 已完成",
    description: "清理项目构建产物（node_modules、target、DerivedData 等）。",
    implemented: true,
  },
  {
    id: "history",
    label: "历史记录",
    icon: "🕘",
    origin: "Mole bin/history.sh + lib/core/history.sh",
    status: "模块 8 · 待迁移",
    description: "查看清理与卸载操作日志。",
  },
  {
    id: "settings",
    label: "设置",
    icon: "⚙️",
    origin: "Mole lib/manage/*",
    status: "模块 8 · 待迁移",
    description: "白名单保护、应用更新与偏好设置。",
  },
];

const activeId = ref("status");
const active = computed(
  () => modules.find((m) => m.id === activeId.value) ?? modules[0],
);
</script>

<template>
  <div class="shell">
    <aside class="sidebar">
      <div class="brand">
        <span class="logo">🐹</span>
        <span class="name">Mole RS</span>
      </div>
      <nav>
        <button
          v-for="m in modules"
          :key="m.id"
          class="nav-item"
          :class="{ active: m.id === activeId }"
          @click="activeId = m.id"
        >
          <span class="icon">{{ m.icon }}</span>
          <span class="label">{{ m.label }}</span>
          <span v-if="!m.implemented" class="dot" title="迁移中" />
        </button>
      </nav>
      <footer class="sidebar-foot">对标 tw93/mole · GPL-3.0</footer>
    </aside>

    <main class="content">
      <StatusPage v-if="activeId === 'status'" />
      <CleanPage v-else-if="activeId === 'clean'" />
      <PurgePage v-else-if="activeId === 'purge'" />
      <PagePlaceholder v-else v-bind="active" />
    </main>
  </div>
</template>

<style scoped>
.shell {
  display: flex;
  height: 100vh;
  overflow: hidden;
}

.sidebar {
  flex: 0 0 200px;
  display: flex;
  flex-direction: column;
  background: var(--surface);
  border-right: 1px solid var(--border);
  padding: 16px 10px;
  user-select: none;
}

.brand {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 10px 16px;
}

.logo {
  font-size: 20px;
}

.name {
  font-weight: 600;
  font-size: 15px;
  letter-spacing: 0.2px;
}

nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 10px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text);
  font-size: 13.5px;
  font-family: inherit;
  text-align: left;
  cursor: pointer;
}

.nav-item:hover {
  background: var(--surface-inset);
}

.nav-item.active {
  background: var(--accent);
  color: #fff;
}

.icon {
  font-size: 15px;
  width: 20px;
  text-align: center;
}

.dot {
  margin-left: auto;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--warning);
}

.nav-item.active .dot {
  background: rgba(255, 255, 255, 0.8);
}

.sidebar-foot {
  margin-top: auto;
  padding: 10px;
  font-size: 11px;
  color: var(--text-secondary);
}

.content {
  flex: 1;
  overflow: auto;
  background: var(--bg);
}
</style>
