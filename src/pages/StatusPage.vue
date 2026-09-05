<script setup lang="ts">
/**
 * 系统监控页，对标 Mole `cmd/status` 的 TUI 视图（view.go）。
 *
 * 数据契约与 Go `MetricsSnapshot` JSON 完全一致（snake_case 字段），
 * 刷新节奏由后端 Collector 状态机维持（对标 watchState），前端每秒
 * 调用一次 status_tick 即可。
 */
import { computed, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watchEffect } from "vue";
import { invoke } from "@tauri-apps/api/core";

interface CpuStatus {
  usage: number;
  per_core: number[];
  per_core_estimated: boolean;
  load1: number;
  load5: number;
  load15: number;
  core_count: number;
  logical_cpu: number;
  p_core_count: number;
  e_core_count: number;
}
interface MemoryStatus {
  used: number;
  total: number;
  available: number;
  used_percent: number;
  swap_used: number;
  swap_total: number;
  cached: number;
  pressure: string;
}
interface DiskStatus {
  mount: string;
  device: string;
  used: number;
  total: number;
  used_percent: number;
  fstype: string;
  external: boolean;
  smart_status: string;
  purgeable: number;
}
interface NetworkStatus {
  name: string;
  rx_rate_mbs: number;
  tx_rate_mbs: number;
  ip: string;
}
interface BatteryStatus {
  percent: number;
  status: string;
  time_left: string;
  health: string;
  cycle_count: number;
  capacity: number;
}
interface ProcessInfo {
  pid: number;
  name: string;
  command: string;
  cpu: number;
  memory_bytes: number;
}
interface HardwareInfo {
  model: string;
  cpu_model: string;
  total_ram: string;
  disk_size: string;
  os_version: string;
  refresh_rate: string;
}
interface GpuStatus {
  name: string;
  usage: number;
  core_count: number;
  note: string;
}
interface BluetoothDevice {
  name: string;
  connected: boolean;
  battery: string;
}
interface MetricsSnapshot {
  collected_at: number;
  host: string;
  platform: string;
  uptime: string;
  uptime_seconds: number;
  procs: number;
  hardware: HardwareInfo;
  health_score: number;
  health_score_msg: string;
  cpu: CpuStatus;
  gpu: GpuStatus[];
  memory: MemoryStatus;
  disks: DiskStatus[];
  network: NetworkStatus[];
  network_history: { rx_history: number[]; tx_history: number[] };
  proxy: { enabled: boolean; type: string; host: string };
  batteries: BatteryStatus[];
  bluetooth: BluetoothDevice[];
  thermal: {
    fan_speed: number;
    system_power: number;
    adapter_power: number;
    battery_power: number;
  };
  top_processes: ProcessInfo[];
  zombie_count: number | null;
  process_stale: boolean | null;
}

const EMPTY: MetricsSnapshot = {
  collected_at: 0,
  host: "",
  platform: "",
  uptime: "",
  uptime_seconds: 0,
  procs: 0,
  hardware: {
    model: "",
    cpu_model: "",
    total_ram: "",
    disk_size: "",
    os_version: "",
    refresh_rate: "",
  },
  health_score: 0,
  health_score_msg: "",
  gpu: [],
  cpu: {
    usage: 0,
    per_core: [],
    per_core_estimated: false,
    load1: 0,
    load5: 0,
    load15: 0,
    core_count: 0,
    logical_cpu: 0,
    p_core_count: 0,
    e_core_count: 0,
  },
  memory: {
    used: 0,
    total: 0,
    available: 0,
    used_percent: 0,
    swap_used: 0,
    swap_total: 0,
    cached: 0,
    pressure: "",
  },
  disks: [],
  network: [],
  network_history: { rx_history: [], tx_history: [] },
  proxy: { enabled: false, type: "", host: "" },
  batteries: [],
  bluetooth: [],
  thermal: { fan_speed: 0, system_power: 0, adapter_power: 0, battery_power: 0 },
  top_processes: [],
  zombie_count: null,
  process_stale: null,
};

const snap = ref<MetricsSnapshot>(EMPTY);
let timer: number | undefined;

async function tick() {
  try {
    snap.value = await invoke<MetricsSnapshot>("status_tick");
  } catch (e) {
    console.error("status_tick failed", e);
  }
}

function start() {
  if (timer !== undefined) return;
  void tick();
  timer = window.setInterval(tick, 1000);
}
function stop() {
  if (timer !== undefined) {
    window.clearInterval(timer);
    timer = undefined;
  }
}

// 页面被 KeepAlive 缓存：切出标签页时暂停每秒采集（不让后台页持续
// 占用后端采集），切回立即恢复一帧。
onMounted(start);
onActivated(start);
onDeactivated(stop);
onBeforeUnmount(stop);

/** 与后端一致的二进制单位格式化（经 Rust 单一实现，对标 units.BytesBin）。 */
async function bin(bytes: number): Promise<string> {
  try {
    return await invoke<string>("format_bytes_bin", { v: bytes });
  } catch {
    return `${bytes} B`;
  }
}

const memUsedText = ref("…");
const memTotalText = ref("…");
const memCachedText = ref("…");
const memSwapText = ref("…");
watchEffect(async () => {
  const m = snap.value.memory;
  memUsedText.value = await bin(m.used);
  memTotalText.value = await bin(m.total);
  memCachedText.value = await bin(m.cached);
  memSwapText.value = m.swap_total
    ? `${await bin(m.swap_used)} / ${await bin(m.swap_total)}`
    : "—";
});

const healthColor = computed(() => {
  const s = snap.value.health_score;
  if (s >= 85) return "var(--ok)";
  if (s >= 65) return "var(--accent)";
  if (s >= 45) return "var(--warning)";
  return "var(--danger)";
});

function diskColor(pct: number): string {
  if (pct >= 93) return "var(--danger)";
  if (pct >= 80) return "var(--warning)";
  return "var(--accent)";
}

/** 网络历史 sparkline 的 SVG 折线点。 */
function sparkPoints(history: number[], max: number): string {
  const h = 28;
  if (history.length < 2) return "";
  const peak = Math.max(max, 0.01);
  return history
    .map((v, i) => {
      const x = (i / (history.length - 1)) * 100;
      const y = h - Math.min(v / peak, 1) * h;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
}
const netPeak = computed(() => {
  const { rx_history, tx_history } = snap.value.network_history;
  return Math.max(0.01, ...rx_history, ...tx_history);
});
const battery = computed(() => snap.value.batteries[0]);
</script>

<template>
  <section class="status-page">
    <header class="page-head">
      <div class="health">
        <div class="score-ring" :style="{ borderColor: healthColor }">
          <span class="score">{{ snap.health_score }}</span>
        </div>
        <div>
          <h2>{{ snap.health_score_msg || "采集中…" }}</h2>
          <p class="sub">
            {{ snap.host }} · {{ snap.platform }} · 开机 {{ snap.uptime }} ·
            {{ snap.procs }} 进程
          </p>
        </div>
      </div>
      <div class="hw">
        <p class="model">{{ snap.hardware.model }}</p>
        <p class="sub">
          {{ snap.hardware.cpu_model }} · {{ snap.hardware.total_ram }} ·
          {{ snap.hardware.disk_size }}<template v-if="snap.hardware.refresh_rate">
            · {{ snap.hardware.refresh_rate }}</template
          >
        </p>
        <p class="sub">{{ snap.hardware.os_version }}</p>
      </div>
    </header>

    <div class="grid">
      <!-- CPU -->
      <article class="card">
        <h3>CPU</h3>
        <div class="big">{{ snap.cpu.usage.toFixed(1) }}<span class="unit">%</span></div>
        <div class="cores">
          <div
            v-for="(v, i) in snap.cpu.per_core"
            :key="i"
            class="core-bar"
            :title="`CPU${i}: ${v.toFixed(0)}%`"
          >
            <div class="core-fill" :style="{ height: `${v}%` }" />
          </div>
        </div>
        <p class="meta">
          {{ snap.cpu.logical_cpu }} 核<template v-if="snap.cpu.p_core_count">
            · {{ snap.cpu.p_core_count }}P+{{ snap.cpu.e_core_count }}E</template
          >
          · 负载 {{ snap.cpu.load1.toFixed(2) }} / {{ snap.cpu.load5.toFixed(2) }} /
          {{ snap.cpu.load15.toFixed(2) }}
        </p>
      </article>

      <!-- Memory -->
      <article class="card">
        <h3>内存</h3>
        <div class="big">
          {{ snap.memory.used_percent.toFixed(1) }}<span class="unit">%</span>
        </div>
        <div class="bar">
          <div
            class="bar-fill"
            :style="{
              width: `${Math.min(snap.memory.used_percent, 100)}%`,
              background: diskColor(snap.memory.used_percent),
            }"
          />
        </div>
        <p class="meta">
          {{ memUsedText }} / {{ memTotalText }} · 缓存 {{ memCachedText }} · 交换
          {{ memSwapText }}
          <template v-if="snap.memory.pressure"> · 压力 {{ snap.memory.pressure }}</template>
        </p>
      </article>

      <!-- Disks -->
      <article class="card">
        <h3>磁盘</h3>
        <div v-for="d in snap.disks" :key="d.mount" class="disk-row">
          <div class="disk-label">
            <span>{{ d.mount }}</span>
            <span class="sub">{{ d.used_percent.toFixed(1) }}%</span>
          </div>
          <div class="bar">
            <div
              class="bar-fill"
              :style="{
                width: `${Math.min(d.used_percent, 100)}%`,
                background: diskColor(d.used_percent),
              }"
            />
          </div>
        </div>
        <p class="meta" v-if="snap.disks[0]">
          {{ snap.disks[0].fstype.toUpperCase() }} · {{ snap.disks[0].smart_status }}
        </p>
      </article>

      <!-- Network -->
      <article class="card">
        <h3>网络</h3>
        <div class="net-spark">
          <svg viewBox="0 0 100 28" preserveAspectRatio="none">
            <polyline
              :points="sparkPoints(snap.network_history.rx_history, netPeak)"
              fill="none"
              stroke="var(--accent)"
              stroke-width="1.5"
            />
            <polyline
              :points="sparkPoints(snap.network_history.tx_history, netPeak)"
              fill="none"
              stroke="var(--ok)"
              stroke-width="1.5"
            />
          </svg>
          <span class="legend"><i class="rx" />↓ <i class="tx" />↑</span>
        </div>
        <div v-for="n in snap.network" :key="n.name" class="net-row">
          <span class="net-name">{{ n.name }}</span>
          <span class="sub mono">{{ n.ip || "—" }}</span>
          <span class="net-rate">
            ↓{{ n.rx_rate_mbs.toFixed(2) }} ↑{{ n.tx_rate_mbs.toFixed(2) }} MB/s
          </span>
        </div>
        <p class="meta" v-if="snap.proxy.enabled">
          {{ snap.proxy.type }} · {{ snap.proxy.host }}
        </p>
      </article>

      <!-- GPU -->
      <article class="card" v-if="snap.gpu.length">
        <h3>GPU</h3>
        <div v-for="g in snap.gpu" :key="g.name" class="disk-row">
          <div class="disk-label">
            <span>{{ g.name }}</span>
            <span class="sub">
              {{ g.usage >= 0 ? `${g.usage.toFixed(0)}%` : "需 root" }}
              <template v-if="g.core_count"> · {{ g.core_count }} 核</template>
            </span>
          </div>
          <div v-if="g.usage >= 0" class="bar">
            <div
              class="bar-fill"
              :style="{ width: `${Math.min(g.usage, 100)}%`, background: 'var(--accent)' }"
            />
          </div>
          <p class="meta" v-if="g.note">{{ g.note }}</p>
        </div>
      </article>

      <!-- Bluetooth -->
      <article class="card" v-if="snap.bluetooth.length">
        <h3>蓝牙设备</h3>
        <div v-for="(d, i) in snap.bluetooth" :key="`${d.name}-${i}`" class="net-row">
          <span class="net-name">{{ d.name }}</span>
          <span class="sub">{{ d.connected ? "已连接" : "未连接" }}</span>
          <span class="net-rate" v-if="d.battery">🔋 {{ d.battery }}</span>
        </div>
      </article>

      <!-- Battery / Power -->
      <article class="card" v-if="battery">
        <h3>电池与功耗</h3>
        <div class="big">{{ battery.percent.toFixed(0) }}<span class="unit">%</span></div>
        <p class="meta">
          {{ battery.status }}<template v-if="battery.time_left && battery.time_left !== '0:00'">
            · 剩余 {{ battery.time_left }}</template
          >
          · 健康 {{ battery.health }}
          <template v-if="battery.capacity"> · 容量 {{ battery.capacity }}%</template>
          <template v-if="battery.cycle_count"> · {{ battery.cycle_count }} 次循环</template>
        </p>
        <p
          class="meta"
          v-if="snap.thermal.fan_speed || snap.thermal.system_power || snap.thermal.adapter_power"
        >
          <template v-if="snap.thermal.fan_speed">风扇 {{ snap.thermal.fan_speed }} rpm · </template>
          <template v-if="snap.thermal.system_power">
            系统 {{ snap.thermal.system_power.toFixed(1) }}W · </template
          >
          <template v-if="snap.thermal.adapter_power">
            适配器 {{ snap.thermal.adapter_power.toFixed(0) }}W</template
          >
        </p>
      </article>

      <!-- Top processes -->
      <article class="card wide">
        <h3>Top 进程</h3>
        <table class="procs">
          <thead>
            <tr>
              <th>进程</th>
              <th>PID</th>
              <th class="num">CPU%</th>
              <th class="num">内存</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="p in snap.top_processes" :key="p.pid">
              <td class="proc-name" :title="p.command">{{ p.name }}</td>
              <td class="mono">{{ p.pid }}</td>
              <td class="num">{{ p.cpu.toFixed(1) }}</td>
              <td class="num mono">
                {{ p.memory_bytes ? Math.round(p.memory_bytes / 1048576) + " MB" : "—" }}
              </td>
            </tr>
            <tr v-if="!snap.top_processes.length">
              <td colspan="4" class="sub">采集中…</td>
            </tr>
          </tbody>
        </table>
        <p class="meta" v-if="snap.zombie_count">僵尸进程 {{ snap.zombie_count }}</p>
      </article>
    </div>
  </section>
</template>

<style scoped>
.status-page {
  padding: 24px 28px;
  max-width: 1080px;
  margin: 0 auto;
}

.page-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 16px;
  margin-bottom: 20px;
}

.health {
  display: flex;
  gap: 14px;
  align-items: center;
}

.health h2 {
  margin: 0;
  font-size: 17px;
}

.score-ring {
  width: 64px;
  height: 64px;
  border-radius: 50%;
  border: 4px solid;
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
}

.score {
  font-size: 20px;
  font-weight: 700;
}

.sub {
  margin: 2px 0 0;
  color: var(--text-secondary);
  font-size: 12px;
}

.hw {
  text-align: right;
}

.model {
  margin: 0;
  font-weight: 600;
  font-size: 14px;
}

.grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
  gap: 14px;
}

.card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 16px 18px;
}

.card.wide {
  grid-column: 1 / -1;
}

.card h3 {
  margin: 0 0 10px;
  font-size: 13px;
  color: var(--text-secondary);
  font-weight: 600;
  letter-spacing: 0.4px;
}

.big {
  font-size: 30px;
  font-weight: 700;
  line-height: 1.1;
}

.unit {
  font-size: 15px;
  color: var(--text-secondary);
  margin-left: 2px;
}

.cores {
  display: flex;
  gap: 3px;
  align-items: flex-end;
  height: 42px;
  margin: 10px 0 8px;
}

.core-bar {
  flex: 1;
  height: 100%;
  background: var(--surface-inset);
  border-radius: 2px;
  display: flex;
  align-items: flex-end;
  overflow: hidden;
}

.core-fill {
  width: 100%;
  background: var(--accent);
  border-radius: 2px;
  transition: height 0.4s ease;
}

.bar {
  height: 8px;
  background: var(--surface-inset);
  border-radius: 4px;
  overflow: hidden;
  margin: 8px 0;
}

.bar-fill {
  height: 100%;
  border-radius: 4px;
  transition: width 0.4s ease;
}

.meta {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--text-secondary);
}

.disk-row {
  margin-bottom: 10px;
}

.disk-label {
  display: flex;
  justify-content: space-between;
  font-size: 12.5px;
  margin-bottom: 2px;
}

.net-spark {
  display: flex;
  align-items: center;
  gap: 8px;
}

.net-spark svg {
  flex: 1;
  height: 34px;
}

.legend {
  font-size: 11px;
  color: var(--text-secondary);
}

.legend i {
  display: inline-block;
  width: 10px;
  height: 3px;
  border-radius: 2px;
  vertical-align: middle;
  margin: 0 2px 0 6px;
}

.legend .rx {
  background: var(--accent);
}

.legend .tx {
  background: var(--ok);
}

.net-row {
  display: flex;
  align-items: baseline;
  gap: 10px;
  font-size: 12.5px;
  margin-top: 8px;
}

.net-name {
  font-weight: 600;
  width: 60px;
}

.net-rate {
  margin-left: auto;
  font-size: 12px;
}

.procs {
  width: 100%;
  border-collapse: collapse;
  font-size: 12.5px;
}

.procs th {
  text-align: left;
  color: var(--text-secondary);
  font-weight: 500;
  padding: 4px 8px 6px 0;
  border-bottom: 1px solid var(--border);
}

.procs td {
  padding: 5px 8px 5px 0;
  border-bottom: 1px solid var(--border);
}

.procs .num {
  text-align: right;
}

.proc-name {
  max-width: 340px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mono {
  font-family: "SF Mono", Menlo, monospace;
  font-size: 11.5px;
}
</style>
