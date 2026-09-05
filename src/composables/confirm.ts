/**
 * 应用内确认对话框（promise 风格）。
 *
 * 为什么不用 window.confirm：Tauri 的 WKWebView（wry）未实现
 * WKUIDelegate 的 JS 对话框方法，WebKit 对未实现的 confirm/alert/prompt
 * 一律按"用户取消"处理——window.confirm() 恒返回 false，导致清理/删除
 * 操作永远无法确认。对标终端 `mo` 删除前的 y/N 确认，改用应用内模态框。
 */
import { readonly, ref } from "vue";

const open = ref(false);
const message = ref("");
const title = ref("确认操作");
const confirmText = ref("确认");
const danger = ref(true);

let resolver: ((ok: boolean) => void) | null = null;

export function confirm(
  msg: string,
  opts?: { title?: string; confirmText?: string; danger?: boolean },
): Promise<boolean> {
  message.value = msg;
  title.value = opts?.title ?? "确认操作";
  confirmText.value = opts?.confirmText ?? "确认";
  danger.value = opts?.danger ?? true;
  open.value = true;
  return new Promise((resolve) => {
    resolver = resolve;
  });
}

/** 仅供 ConfirmDialog 组件回调用。 */
export function settleConfirm(ok: boolean) {
  open.value = false;
  resolver?.(ok);
  resolver = null;
}

export const confirmState = readonly({ open, message, title, confirmText, danger });
