import AppKit
import ApplicationServices
import Foundation

// 读一个 GUI 进程的系统无障碍控件树（macOS AXUIElement）。
// 用法：axwalk <pid> [关键字...] [press=<子串>]  —— press= 会按下第一个匹配的元素（点会话行用）。
let args = CommandLine.arguments
guard args.count >= 2, let pid = Int32(args[1]) else {
    FileHandle.standardError.write("用法: axwalk <pid> [关键字...]\n".data(using: .utf8)!)
    exit(2)
}
let rawArgs = Array(args.dropFirst(2))
var pressSub: String? = nil
var keywords: [String] = []
var moveToScreen = false
for a in rawArgs {
    if a.hasPrefix("press=") { pressSub = String(a.dropFirst(6)) }
    else if a == "move=onscreen" { moveToScreen = true }
    else { keywords.append(a) }
}
var pressed = 0

func str(_ el: AXUIElement, _ attr: String) -> String? {
    var v: CFTypeRef?
    guard AXUIElementCopyAttributeValue(el, attr as CFString, &v) == .success, let raw = v else { return nil }
    // 只按 CF 类型 ID 认串与数字：AXValue / AXArray 之类的对象直接 `as?` 桥接会段错误（实测 139）
    let tid = CFGetTypeID(raw)
    if tid == CFStringGetTypeID() { return (raw as! CFString) as String }
    if tid == CFNumberGetTypeID() { return "\(raw as! CFNumber)" }
    if tid == CFBooleanGetTypeID() { return (raw as! CFBoolean) == kCFBooleanTrue ? "true" : "false" }
    return nil
}

func children(_ el: AXUIElement) -> [AXUIElement] {
    var v: CFTypeRef?
    guard AXUIElementCopyAttributeValue(el, kAXChildrenAttribute as CFString, &v) == .success,
          let raw = v, CFGetTypeID(raw) == CFArrayGetTypeID(),
          let arr = (raw as! CFArray) as? [AXUIElement] else { return [] }
    return arr
}

let app = AXUIElementCreateApplication(pid)
// 裸二进制（不是 .app bundle）启动时，进程可能是 prohibited 状态：CGWindowList 看得到窗口，
// AX 侧却只给菜单栏。先试着把它拉回常规状态。
if let ra = NSRunningApplication(processIdentifier: pid) {
    print("激活前 activationPolicy=\(ra.activationPolicy.rawValue) hidden=\(ra.isHidden)")
    ra.unhide()
    ra.activate(options: [.activateAllWindows])
    usleep(600_000)
    print("激活后 activationPolicy=\(ra.activationPolicy.rawValue)")
}
// WKWebView 只在"检测到辅助技术"时才建 AX 树；先把这两个开关打开再读。
// 实测：单次设置会返回 -25205（attributeUnsupported），要重试几次才吃得住。
var manual: AXError = .success
var enhanced: AXError = .success
for i in 0..<8 {
    manual = AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)
    enhanced = AXUIElementSetAttributeValue(app, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
    if manual == .success || enhanced == .success { print("第 \(i + 1) 次设成功"); break }
    usleep(350_000)
}
print("设 AXManualAccessibility=\(manual)  设 AXEnhancedUserInterface=\(enhanced)")

var winsRef: CFTypeRef?
let wrc = AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &winsRef)
// 不是数组就当作"没有窗口"：把单个元素桥接成 1 元素数组会让遍历从 AXApplication 自环（实测）
let wins: [AXUIElement] = {
    guard wrc == .success, let raw = winsRef, CFGetTypeID(raw) == CFArrayGetTypeID() else { return [] }
    return ((raw as! CFArray) as? [AXUIElement]) ?? []
}()
print("AXWindows rc=\(wrc) 数量=\(wins.count)")

var total = 0
var hist: [String: Int] = [:]
var lines: [String] = []
var matched: [String] = []

func walk(_ el: AXUIElement, depth: Int) {
    if total > 30_000 { return }
    if depth > 60 { return }   // 深树递归会撑爆栈（实测 SIGSEGV/139）
    total += 1
    let role = str(el, kAXRoleAttribute as String) ?? "?"
    // 窗口元素里会回指 AXApplication 自己（实测：照原样下钻会自环 60 层再吃掉整个菜单栏）
    if depth > 0 && role == "AXApplication" { return }
    let sub = str(el, kAXSubroleAttribute as String) ?? ""
    let desc = str(el, kAXDescriptionAttribute as String) ?? ""
    let val = str(el, kAXValueAttribute as String) ?? ""
    let title = str(el, kAXTitleAttribute as String) ?? ""
    let help = str(el, kAXHelpAttribute as String) ?? ""
    let line = "\(String(repeating: "  ", count: depth))[\(role)/\(sub)] desc=\"\(desc)\" val=\"\(val)\" title=\"\(title)\" help=\"\(help)\""
    if lines.count < 120 { lines.append(line) }
    let hay = "\(desc) \(val) \(title) \(help)"
    hist[role, default: 0] += 1
    if let p = pressSub, hay.contains(p), pressed < 1 {
        pressed += 1
        let rc = AXUIElementPerformAction(el, "AXPress" as CFString)
        print("按下 \"\(p)\" → \(rc)")
    }
    if keywords.contains(where: { hay.contains($0) }) && matched.count < 4000 {
        matched.append(line)
    }
    // 系统菜单栏整个子树跳过：它会把关键字额度吃光，而且不是这一轮要判的东西
    if role == "AXMenuBar" || role == "AXMenu" { return }
    for c in children(el) { walk(c, depth: depth + 1) }
}

for (i, w) in wins.enumerated() {
    if moveToScreen {
        var pt = CGPoint(x: 60, y: 80)
        var sz = CGSize(width: 1024, height: 780)
        let pv = AXValueCreate(.cgPoint, &pt)
        let sv = AXValueCreate(.cgSize, &sz)
        var rcp = -1, rcs = -1
        if let p = pv { rcp = Int(AXUIElementSetAttributeValue(w, kAXPositionAttribute as CFString, p).rawValue) }
        if let s = sv { rcs = Int(AXUIElementSetAttributeValue(w, kAXSizeAttribute as CFString, s).rawValue) }
        let rfr = Int(AXUIElementSetAttributeValue(w, kAXMainAttribute as CFString, kCFBooleanTrue).rawValue)
        let rtf = Int(AXUIElementSetAttributeValue(w, kAXFocusedAttribute as CFString, kCFBooleanTrue).rawValue)
        print("window[\(i)] 挪回屏内 rc: pos=\(rcp) size=\(rcs) main=\(rfr) focused=\(rtf)")
        usleep(400_000)
    }
    print("── window[\(i)] title=\"\(str(w, kAXTitleAttribute as String) ?? "")\"")
    walk(w, depth: 0)
}
print("遍历到的元素总数=\(total)")
print("角色直方图：" + hist.sorted { $0.value > $1.value }.map { "\($0.0)=\($0.1)" }.joined(separator: " "))
if keywords.isEmpty {
    lines.forEach { print($0) }
} else {
    print("命中关键字的行数=\(matched.count)")
    matched.prefix(80).forEach { print($0) }
}
