<script setup lang="ts">
/**
 * 本人头像的**唯一**渲染处（用户 2026-10-07：「没上传时应该都是同一套默认 emoji 头像，
 * 这些应该是同一个组件取的同一个数据源」）。
 *
 * 为什么要收成一个：此前"有图显示图、没图显示什么"这句话在四处各写了一遍，
 * 而"没图显示什么"三处答案不一样 —— 桌面导航栏是 emoji 小动物、移动端"我的"页顶部是
 * lucide 的 `UserCircle`（一个蓝灰色人形图标）、资料页虽是 emoji 但被一层 `opacity-0`
 * 的红色相机蒙版盖住，而 `@media (hover: none)` 会把那层蒙版强制成常显
 * （触屏没有 hover，见 style.css 的悬停揭示兜底那段）⇒ 手机上根本看不见头像本体。
 * 同一台设备、同一个账号，三张脸。
 *
 * 数据源也一并收成一个：图 = `app.device.avatar`，脸 = `avatarSeedFor(app.device.device_id)`。
 * ⚠️ 种子**不许**用输入框里那份未保存的 nickname：改一次昵称就换一次脸，
 * 而导航栏读的是 `app.device.nickname` ⇒ 保存之前两处不同脸（这正是要修的病）。
 *
 * 尺寸与圆角留给调用点（class 透传到根节点）：这几处本来就是一大一小的层级关系，
 * 写死只会逼调用点再套一层壳。emoji 字号是 `58cqw`（容器查询），
 * 所以只要 `gosslan-avatar-box` 与尺寸类落在同一个元素上，16px 与 64px 共用一份定义。
 *
 * ⚠️ 这里**不留插槽**给"压在头像上的那一层"（在线点、相机角标）：根节点带
 * `overflow-hidden`（圆角裁切用），往里塞负偏移的角标会被裁掉。那一层一律做成
 * 本组件的**兄弟节点**、挂在同一个 `relative` 父级上 —— 桌面导航栏的在线点原本就是这么放的。
 *
 * `previewSrc` 是给"换头像"那个编辑器用的**乐观预览**：选完图先让用户看见换成了这张，
 * 落库失败再回滚（那段回滚在 `settings/ProfileSection.vue` 里）。它只覆盖**图片**，
 * 默认脸的种子始终来自 store ⇒ 乐观态撤掉之后落回的那张脸和别处仍然一致。
 * 判据是"这个 prop 有没有被传"（`undefined` = 没传），不是"值是否为空"：
 * 传 `null` 是编辑器在说"当前没有头像"，那也得听它的。
 */
import { computed } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { avatarSeedFor } from "@/utils/avatarSeed";

const props = defineProps<{
  /** 乐观预览用的图片地址；不传则读 `app.device.avatar`。 */
  previewSrc?: string | null;
}>();

const app = useAppStore();
const src = computed(() =>
  props.previewSrc !== undefined ? props.previewSrc : (app.device?.avatar ?? null),
);
const seed = computed(() =>
  avatarSeedFor(app.device?.device_id || app.device?.nickname || null),
);
</script>

<template>
  <div
    class="gosslan-avatar-box relative flex items-center justify-center overflow-hidden text-white"
    :style="{ backgroundColor: seed?.bg }"
  >
    <img alt="" draggable="false" v-if="src" :src="src" class="h-full w-full object-cover" />
    <span v-else class="gosslan-avatar-emoji" aria-hidden="true">{{ seed?.emoji }}</span>
  </div>
</template>
