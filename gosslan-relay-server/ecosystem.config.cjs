/**
 * PM2 配置（单台服务器长期运行的推荐形态）。
 *
 * 为什么是 `.cjs`：本目录的 `package.json` 声明了 `"type": "module"`（`server.mjs` 是 ESM），
 * 而 PM2 的配置文件走 `require()`。用 `.cjs` 后缀可以显式压过 `"type": "module"`，
 * 否则 PM2 会报 `require is not defined in ES module scope`。
 *
 * 启动：
 *   npm i -g pm2          # PM2 是部署工具，不是本程序的运行依赖（程序本身零依赖）
 *   pm2 start ecosystem.config.cjs
 *   pm2 save
 *   pm2 startup systemd -u <运行用户> --hp /home/<运行用户>   # 开机自启
 *   pm2 logs gosslan-relay
 * 细节（升级、回滚、日志切割、故障排查）见 README「用 PM2 长期运行」。
 *
 * ## ⚠️ instances 必须是 1，且不得开 cluster 模式
 * 配对用的「通道哈希 → 等待中的连接」这张表是**进程内内存**。cluster 会 fork 出多个
 * worker，各自一张表，而两条客户端连接会被内核算法分到不同 worker ⇒ **永远配不上对**。
 * 故障形态最难查：两端都显示"已连上服务器"，日志也没有任何错误，就是连不成。
 *
 * 单进程够用吗？实测（见 README 实测段）15 条电路搬运 202MiB/s、p50 0.16ms，
 * RSS 55MiB —— 瓶颈在公网 RTT，不在这台进程。真要扩规模，正确做法是**多开几台服务器、
 * 客户端多配一台**，而不是在一台机器上开 cluster。
 */

const fs = require('node:fs');
const path = require('node:path');

/**
 * 口令来源，按优先级：环境变量 TOKEN → /etc/gosslan-relay.env → 同目录 .env。
 * 文件格式就是一行 `TOKEN=xxxx`。
 *
 * 口令不写进本仓库、也不写进 PM2 配置：`pm2 describe` 会把 env 原样打印出来，
 * 而拿到口令的人可以用这台服务器转发任意字节。
 */
function readToken() {
  if (process.env.TOKEN) return process.env.TOKEN;
  const candidates = ['/etc/gosslan-relay.env', path.join(__dirname, '.env')];
  for (const file of candidates) {
    if (!fs.existsSync(file)) continue;
    const line = fs
      .readFileSync(file, 'utf8')
      .split('\n')
      .map((l) => l.trim())
      .find((l) => l.startsWith('TOKEN='));
    if (line) return line.slice('TOKEN='.length).replace(/^["']|["']$/g, '');
  }
  // 返回空串 ⇒ server.mjs 以退出码 2 拒绝启动，不会退化成开放代理。
  return '';
}

module.exports = {
  apps: [
    {
      name: 'gosslan-relay',
      script: './server.mjs',
      cwd: __dirname,

      // 单进程 fork：见文件头，这是功能正确性问题，不是性能偏好。
      exec_mode: 'fork',
      instances: 1,

      // 崩溃自动拉起；退避 3s 避免配置错时刷屏重启。
      autorestart: true,
      restart_delay: 3000,
      max_restarts: 20,
      min_uptime: '10s',
      // 内存兜底：正常 RSS 几十 MiB，超阈值即视为泄漏 —— 重启比拖垮整机好。
      max_memory_restart: '160M',
      kill_timeout: 5000, // 留时间给 SIGTERM 处理器把两端电路干净关掉

      env: {
        NODE_ENV: 'production',
        TOKEN: readToken(),
        PORT: '59993',
        HOST: '0.0.0.0',
        MAX_CLIENTS: '128',
        LOG: '1',
        STATS: '1',
      },

      // 日志只有事件行（接入/配对/拒绝/统计），不含任何载荷。
      out_file: './logs/out.log',
      error_file: './logs/error.log',
      merge_logs: true,
      time: true, // 每行前加时间戳，不依赖 journald 也能看时序
    },
  ],
};
