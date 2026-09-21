import { t } from "@/i18n";
import { computed, toValue, type CSSProperties, type MaybeRefOrGetter } from "vue";
import dayjs from "dayjs";
import { useAppStore } from "@/stores/useAppStore";
import { findPreset, formatTimeDivider, parsePeerStyle, resolveChatColors } from "@/utils/chatStyle";
import type { MessageRecord } from "@/types";

export type SendState = "sending" | "sent" | "delivered" | "read" | "failed";

/** 时间分割线阈值：与上一条间隔 ≥ 5 分钟。 */
const TIME_DIVIDER_GAP = 5 * 60 * 1000;

/**
 * 消息外观与版面判定（气泡配色、时间行、发送状态）。
 * MessageItem 与其子组件共用同一份判定，避免两边各写一套导致布局错位。
 */
export function useMessageDisplay(opts: {
  message: MaybeRefOrGetter<MessageRecord>;
  prev: MaybeRefOrGetter<MessageRecord | null | undefined>;
  isGroup: MaybeRefOrGetter<boolean>;
}) {
  const app = useAppStore();
  const message = computed(() => toValue(opts.message));
  const prev = computed(() => toValue(opts.prev) ?? null);
  const isGroup = computed(() => toValue(opts.isGroup));

  const mine = computed(() => message.value.sender_id === app.device?.device_id);

  /** 我发的消息用我的样式；对方发的优先用对方广播的样式（未同步过则回退本机）。 */
  const preset = computed(() => {
    if (!mine.value) {
      const raw = app.peerStyles[message.value.sender_id];
      if (raw) return findPreset(parsePeerStyle(raw).preset);
    }
    return findPreset(app.chatStyle.preset);
  });
  /** 气泡配色："theme" 预设按当前主题色运行时派生（本机消息跟随我的主题色，
   *  对方消息按对方广播的偏好渲染），其余预设取表中明/暗值。 */
  const colors = computed(() => resolveChatColors(preset.value.key, app.themeColor, app.dark));
  /** 气泡：圆角/尖角取微信式，配色仍由用户预设决定（--bubble-bg 供尖角取色）。 */
  const bubbleStyle = computed<CSSProperties>(
    () =>
      ({
        "--bubble-bg": mine.value ? colors.value.mineBubble : colors.value.otherBubble,
        background: "var(--bubble-bg)",
        color: mine.value ? colors.value.mineText : colors.value.otherText,
        borderRadius: "var(--gosslan-bubble-radius, 4px)",
        border: mine.value ? "1px solid transparent" : "1px solid var(--gosslan-border)",
        position: "relative",
        // 真实底色（hex），供子组件 mentionHighlightColor 算对比度。
        // CSS 变量字符串无法传入 contrastRatio，必须额外挂一个真实值。
        "--bubble-bg-raw": mine.value ? colors.value.mineBubble : colors.value.otherBubble,
      }) as CSSProperties,
  );

  /**
   * 卡片型气泡（文件/代码）走中性色：学微信——非文本气泡不跟随主题色，
   * 自己发的和对方发的同色，靠左右位置和尖角区分归属，避免满屏都是品牌色。
   * 配色一律取主题 token（--gosslan-card*）：亮色＝浅灰卡片浮在近白画布上（靠 1px 描边区分），
   * 暗色＝比画布亮一档的深灰。**不要在组件里写死这两档色值**——
   * 写死就等于脱离主题，深浅切换时只能靠人肉记住两处都改（这正是本次收敛掉的东西）。
   */
  const cardStyle = computed<CSSProperties>(
    () =>
      ({
        "--bubble-bg": "var(--gosslan-card)",
        background: "var(--bubble-bg)",
        color: "var(--gosslan-card-ink)",
        borderRadius: "var(--gosslan-bubble-radius, 4px)",
        border: "1px solid var(--gosslan-card-line)",
        // 不加阴影：学微信，纯色卡片 + 1px 描边就够，阴影反而显脏
        position: "relative",
      }) as CSSProperties,
  );

  /** 每条消息独立完整渲染（不再合并连续消息）：时间行恒显示。 */
  const showTimeDivider = computed(
    () => !prev.value || message.value.ts - prev.value.ts >= TIME_DIVIDER_GAP,
  );
  const showNickname = computed(() => isGroup.value && !mine.value);

  const fullTime = computed(() => dayjs(message.value.ts).format("YYYY-MM-DD HH:mm:ss"));
  const timeDividerText = computed(() => formatTimeDivider(message.value.ts));

  const sendState = computed(() => message.value.status as SendState);
  const receiptTitle = computed(() => {
    switch (sendState.value) {
      case "sending":
      case "sent":
        return t("msg.sending");
      case "delivered":
        return t("msg.delivered");
      case "read":
        return t("msg.read");
      case "failed":
        return t("msg.sendFailed");
      default:
        return "";
    }
  });

  return {
    mine,
    bubbleStyle,
    cardStyle,
    showTimeDivider,
    showNickname,
    fullTime,
    timeDividerText,
    sendState,
    receiptTitle,
  };
}
