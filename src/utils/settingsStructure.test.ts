/**
 * 设置结构守卫：**导航项的标签必须与点进去看到的内容一致**。
 *
 * 为什么值得单独守：2026-09-12 用户反馈「设置里的分类好像不太合理，通知里面的第一个和
 * 第三个好像不算是通知里的」—— 根因是导航项写着「通知」，点开却是
 * 「语言 + 通知 + 共享目录」（`GeneralSection` 一个组件塞了三件事）。
 * 这类问题**界面上一切正常**（没有报错、功能都在），只有人点进去才会发现分类错了，
 * 所以只能用静态守卫钉住：标签键、分区组件、分区内第一组标题三者必须对得上。
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";
import { test } from "node:test";

const srcDir = join(import.meta.dirname, "..");
const read = (rel: string) => readFileSync(join(srcDir, rel), "utf8");

/** 期望的导航结构：key → 标签键 → 点开后渲染的分区组件（`firstGroupKey` 见下）。 */
const EXPECTED = [
  { key: "profile", labelKey: "settings.group.profile", component: "ProfileSection", firstGroupKey: "settings.group.profile" },
  // 「通用」是**容器**概念（iOS：通用里既有语言与地区也有还原）⇒ 不要求第一组标题同名
  // ⚠️ 2026-09-21 起桌面的「通用」只剩语言与地区（还原独立成「重置」项）——它仍是容器型条目，
  //    所以继续用 null 表达"不要求同名"。
  { key: "general", labelKey: "settings.group.general", component: "GeneralSection", firstGroupKey: null },
  { key: "notifications", labelKey: "settings.group.notifications", component: "NotificationSection", firstGroupKey: "settings.group.notifications" },
  { key: "appearance", labelKey: "settings.item.appearance", component: "AppearanceSection", firstGroupKey: "settings.item.appearance" },
  { key: "network", labelKey: "settings.group.network", component: "NetworkSection", firstGroupKey: "settings.group.network" },
  { key: "files", labelKey: "settings.group.files", component: "FilesSection", firstGroupKey: "settings.group.files" },
  { key: "storage", labelKey: "settings.group.storage", component: "StorageSection", firstGroupKey: "settings.group.storage" },
  { key: "security", labelKey: "settings.group.security", component: "SecuritySection", firstGroupKey: "settings.group.security" },
  // 「关于」/「重置」是两项（用户 2026-09-21：此前一项叫「关于与重置」却只渲染 AboutSection）。
  // 标签统一用**条目名**，且各自与分区内第一组标题同名 —— 这就是本守卫要钉的"标签即内容"。
  { key: "about", labelKey: "settings.item.about", component: "AboutSection", firstGroupKey: "settings.item.about" },
  { key: "reset", labelKey: "settings.group.reset", component: "ResetSection", firstGroupKey: "settings.group.reset" },
] as const;

test("每个导航项都渲染了与标签对应的分区组件（标签即内容）", () => {
  const win = read("components/SettingsWindow.vue");
  for (const item of EXPECTED) {
    assert.match(
      win,
      new RegExp(`\\{ key: "${item.key}", label: t\\("${item.labelKey.replace(/\./g, "\\.")}"\\)`),
      `导航项 ${item.key} 应使用标签键 ${item.labelKey}`,
    );
    // 组件名可能在条件之前（`<ProfileSection v-if="section === 'profile'"`）
    // 也可能之后（`<NotificationSection v-else-if="section === 'notifications'"`），
    // 所以取条件**前后各 250 字符**的窗口来找组件名。
    const at = win.indexOf(`section === '${item.key}'`);
    assert.ok(at >= 0, `SettingsWindow 里没有 ${item.key} 的渲染分支`);
    const window_ = win.slice(Math.max(0, at - 250), at + 250);
    assert.ok(
      window_.includes(item.component),
      `导航项 ${item.key} 点开后必须渲染 ${item.component}（否则"标签说 A、内容是 B"）`,
    );
  }
});

test("分区内第一组标题与导航标签同名（容器型分区除外）", () => {
  for (const item of EXPECTED) {
    if (!item.firstGroupKey) continue;
    const src = read(`components/settings/${item.component}.vue`);
    const titles = [...src.matchAll(/:title="t\('([^']+)'\)"/g)].map((m) => m[1]);
    assert.ok(titles.length > 0, `${item.component} 应有 SettingsGroup 标题`);
    assert.equal(
      titles[0],
      item.firstGroupKey,
      `${item.component} 的第一组标题是 ${titles[0]}，与导航标签 ${item.firstGroupKey} 不一致`,
    );
  }
});

test("每个设置分区组件都被两个外壳引用（没有孤儿分区）", () => {
  const win = read("components/SettingsWindow.vue");
  const panel = read("components/SettingsPanel.vue");
  const files = readdirSync(join(srcDir, "components", "settings")).filter((f) => f.endsWith("Section.vue"));
  for (const f of files) {
    const name = f.replace(".vue", "");
    assert.ok(win.includes(name), `SettingsWindow 未引用 ${name}`);
    assert.ok(panel.includes(name), `SettingsPanel 未引用 ${name}`);
  }
  // 反向：外壳引用的分区组件都必须真实存在（改文件名时不会留死引用）
  for (const item of EXPECTED) {
    assert.ok(files.includes(`${item.component}.vue`), `${item.component}.vue 不存在`);
  }
});

test("「通知」分区里不得混入非通知项（用户反馈的原缺陷）", () => {
  const src = read("components/settings/NotificationSection.vue");
  // 通知分区只应包含通知相关的设置键
  const usedKeys = [...src.matchAll(/t\("(settings\.[^"]+)"\)/g)].map((m) => m[1]);
  for (const key of usedKeys) {
    assert.ok(
      key.startsWith("settings.notify.") || key.startsWith("settings.group.notifications"),
      `NotificationSection 引用了与通知无关的设置项：${key}`,
    );
  }
  // 语言与共享目录必须已经搬走
  assert.ok(!src.includes("settings.language"), "语言不应出现在通知分区");
  assert.ok(!src.includes("share.folder"), "共享目录不应出现在通知分区");
});

/**
 * 资料分区的**头像上传**必须自己落库，且失败要回滚（用户 2026-09-24 #24）。
 *
 * 原缺陷不在"页面不刷新"，而在**保存被静默丢掉**：`onAvatarChange` 直接复用了
 * 昵称那条 `saveProfileNow()`，而它对"昵称为空"是**提前 return** ——
 * 于是"输入框恰好清空 + 点头像换图"这一次上传什么都没做，页面却已经显示新头像，
 * 切个页/重开就弹回旧图。表现完全符合"显示成功但没真成功"，
 * 而界面上没有任何报错，所以只能钉调用形状。
 *
 * 三条判据：① 头像路径不许再走 `saveProfileNow`（那条的提前出口是为昵称设计的）；
 * ② 必须有一处把 `avatar` 改回旧值的回滚（否则失败后页面上挂着从没落库的图）；
 * ③ 昵称为空时以**库里现有的昵称**一起提交，而不是放弃这次上传。
 */
test("资料分区：头像上传自己落库、失败回滚、不受昵称空值牵连", () => {
  const src = read("components/settings/ProfileSection.vue");
  const at = src.indexOf("async function onAvatarChange");
  assert.ok(at > 0, "找不到 onAvatarChange（改名了？）");
  const end = src.indexOf("\nfunction processAvatar", at);
  const body = src.slice(at, end > at ? end : at + 2600);

  // ⚠️ 判"有没有复用"必须匹配**调用形状**：注释里写着 `saveProfileNow()` 是解释为什么不用它，
  // 按裸名字搜会因为注释而误报（与本仓 designGuards 那条"注释里提到类名仍要报"同一条教训）。
  assert.ok(!/await\s+saveProfileNow\(/.test(body), "头像路径不得复用 saveProfileNow —— 它会因昵称为空提前返回");
  // 回滚必须发生在**保存失败那条路径**上。只搜"这一段里有没有 `avatar.value = prev`"是**弱判据**：
  // "昵称为空"那条拒绝分支里也有同样的赋值，把 catch 里那句删掉守卫照样绿
  // （这条实测过，所以要按 catch 块单独切窗口判）。
  const catchAt = body.indexOf("} catch (e) {");
  assert.ok(catchAt > 0, "找不到保存失败的 catch 分支（被改写了？）");
  assert.match(
    body.slice(catchAt, catchAt + 400),
    /avatar\.value = prev/,
    "保存失败必须回滚本地头像，否则页面挂着从没落库的图",
  );
  assert.match(body, /app\.device\?\.nickname/, "昵称为空时以库里现有昵称一起提交");
  // 落库仍只有一个出口：`persistProfile`（两处调用者共用，避免各写一份 updateProfile）
  assert.match(body, /await persistProfile\(name\)/, "头像也要经同一个落库出口");
  const persistAt = src.indexOf("async function persistProfile");
  assert.ok(persistAt > 0, "persistProfile 应存在（单一落库出口）");
  assert.equal(
    (src.match(/app\.updateProfile\(/g) ?? []).length,
    1,
    "updateProfile 只能出现在 persistProfile 里一处 —— 两处各调就会有两份错误处理",
  );
});
