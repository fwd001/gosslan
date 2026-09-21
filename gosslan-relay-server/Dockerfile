# Gosslan 公网中转（哑管道）
#
# 零依赖 ⇒ 镜像里只有 Node 运行时本身，没有 npm install、没有 node_modules。
# 构建：docker build -t gosslan-relay .
# 运行：docker run -d --name gosslan-relay -p 59993:59993 \
#          -e TOKEN="$(openssl rand -hex 16)" --restart unless-stopped gosslan-relay
FROM node:22-alpine

WORKDIR /app
COPY server.mjs ./
# 非 root 运行：这个进程不需要也不应该有任何写权限。
USER node

EXPOSE 59993
# busybox 的 nc 只做 TCP 连通性；不带任何载荷。
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
  CMD nc -z 127.0.0.1 "${PORT:-59993}" || exit 1

# TOKEN 缺失时 server.mjs 会直接以退出码 2 拒绝启动（开放代理不是可接受的默认值）。
CMD ["node", "server.mjs"]
