/**
 * 「这段正文里的 @ 谁算 @到我」的那一份**判定输入**（第二阶段 §9/§10）。
 *
 * 为什么要抽出来：语义本来就已经单源（`utils/linkify`），但它的两个输入 —— 群成员名单
 * 与"我自己是谁" —— 原先只在 `ChatWindow` 里各算一次。任务卡与任务详情也要渲染 @ ⇒
 * 不抽就会长出第二份推导，而 §6① 记的就是这一类（同一个数/同一份判断在两处各算一次）。
 *
 * ⚠️ 两个坑，都踩过或差一点踩：
 *  1. **自己那一项必须取本机昵称** `app.device?.nickname`。
 *     `chat.nicknameOf(我的 device_id)` 查不到 —— 我既不在自己的好友表里、也不在 peers
 *     （那是"别的节点"），会退化成设备指纹 ⇒ 别人 @我 时匹配不上、不高亮；
 *     而 `useMemberProfile().memberProfile(我的 id).name` 的兜底是 `t("common.me")`（「我」），
 *     昵称为空的设备上会拿「我」这个字去匹配 @ 文本 ⇒ 同样匹配不上，表现成"这功能没生效"。
 *  2. 「所有人」要补进名单，否则 @所有人 与 @成员 的高亮样式不一致
 *     （`buildMentionRe` 会去重，真有成员叫这个名字也不会生成重复分支）。
 */
import { computed, type ComputedRef } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { MENTION_ALL_TOKEN } from "@/utils/messages";
import { t } from "@/i18n";

export interface MentionContext {
  mentionNames: ComputedRef<string[]>;
  selfMention: ComputedRef<{ name: string; label: string } | null>;
}

/**
 * @param groupIdOf 当前会话对应的群 id（用函数是为了让它保持"每次求值都跟着活跃会话走"，
 *                  而不是在调用点被拍成一个死值）。非群会话传 null ⇒ 名单为空、不高亮。
 */
export function useMentionContext(groupIdOf: () => string | null | undefined): MentionContext {
  const app = useAppStore();
  const chat = useChatStore();

  const selfMention = computed(() => {
    const name = app.device?.nickname ?? "";
    return name ? { name, label: t("mention.self") } : null;
  });

  const mentionNames = computed(() => {
    const gid = groupIdOf();
    const g = gid ? chat.groups.find((x) => x.id === gid) : null;
    if (!g) return [];
    const me = app.device?.device_id;
    const myName = app.device?.nickname ?? "";
    // 其余成员保持原样（nicknameOf 查不到时回退设备指纹，与插入端行为一致）；
    // 只有"自己"这一项必须换成昵称，否则 @我 永远匹配不上。
    return [
      ...g.members.map((id) => (id === me ? myName || id : chat.nicknameOf(id))),
      MENTION_ALL_TOKEN,
    ];
  });

  return { mentionNames, selfMention };
}
