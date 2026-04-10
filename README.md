# OCI-Sniper-rs

`OCI-Sniper-rs` 是一个基于 Rust 的 Oracle Cloud CLI / Telegram Bot 工具，当前已包含：

- OCI 配置读取与签名请求
- 实例创建载荷建模与默认免费层候选策略
- 本地 CLI：`run`、`test-api`、`bot-webhook`
- 中英文 / 繁中文 i18n 资源与 Telegram 用户语言偏好
- 本地日志轮转、日志打包、最新日志尾部查看
- Telegram `/language`、`/logs [n]`、`/log_latest_tail [n]`
- polling / webhook 两种 Bot 运行模式

## CLI

CLI 支持通过 `--lang` 或 `-l` 切换语言，并输出彩色帮助。

```bash
cargo run -- --lang en --help
cargo run -- --lang zh-CN run --dry-run
cargo run -- -l zh-TW test-api --dump-launch-payload
```

### 命令

- `oci-sniper run`
  读取配置并启动运行时；如果配置了 Telegram token，会按 `telegram.mode` 启动 polling 或 webhook Bot。
- `oci-sniper test-api`
  只执行一次 OCI 已签名 API 测试，不启动 Bot。
- `oci-sniper bot-webhook --set <URL>`
  更新本地配置，并在有 token 时同步调用 Telegram `setWebhook`。
- `oci-sniper bot-webhook --clear`
  清理本地 webhook 配置，并在有 token 时同步调用 Telegram `deleteWebhook`。

## 配置

示例见：

- [config.example.toml](/Users/hsuyelin/Documents/Developer/Github/OCI-Sniper-rs/config.example.toml)：完整模板
- [config.minimal.toml](/Users/hsuyelin/Documents/Developer/Github/OCI-Sniper-rs/config.minimal.toml)：最小 polling 模板
- [config.webhook.toml](/Users/hsuyelin/Documents/Developer/Github/OCI-Sniper-rs/config.webhook.toml)：webhook 模板

### 1. OCI 认证配置

项目配置通过 `[oci]` 指定标准 OCI 配置文件来源：

```toml
[oci]
config_file = "/Users/you/.oci/config"
profile = "DEFAULT"
```

如果不指定 `config_file`，程序会按顺序尝试：

1. `~/.oci/config`
2. `~/.config/oci/config`

兼容的 OCI 配置格式如下：

```ini
[DEFAULT]
user=ocid1.user.oc1..example
fingerprint=aa:bb:cc
tenancy=ocid1.tenancy.oc1..example
region=ap-chuncheon-1
key_file=/Users/you/.oci/oci_api_key.pem
```

### 2. 实例创建配置

支持两种模式：

- 显式配置 `[instance.launch]`
- 不配置 `launch`，使用 `free_tier_defaults` 自动推导默认实例模板

自动模式当前策略是：

1. 从 tenancy compartment 获取可用 AD
2. 选取第一个可用 subnet
3. 按 `shape_candidates` 优先级尝试免费层候选 shape
4. 查询匹配 shape 的 Oracle Linux 可用镜像
5. 自动读取 `~/.ssh/id_ed25519.pub` 或 `~/.ssh/id_rsa.pub`

这是一种“启发式默认值”，不是对所有租户和区域都保证最优。

### 3. Telegram 配置

```toml
[telegram]
bot_token = "123456:ABCDEF"
mode = "polling"
```

#### polling

```toml
[telegram]
bot_token = "123456:ABCDEF"
mode = "polling"
```

#### webhook

```toml
[telegram]
bot_token = "123456:ABCDEF"
mode = "webhook"
webhook_url = "https://your-domain.example/webhook"
webhook_listen = "0.0.0.0:8443"
webhook_path = "/webhook"
```

说明：

- `webhook_url` 是 Telegram 可访问的公网 HTTPS 地址
- `webhook_listen` 是本地监听地址
- `webhook_path` 用于反向代理场景下覆盖内部监听路径
- 如果不设置 `webhook_listen`，会默认回退到 `0.0.0.0:<webhook_url 端口或 8443>`

### 4. Webhook 部署建议

推荐结构：

1. `oci-sniper-rs` 监听本地地址，例如 `127.0.0.1:8443`
2. 由 Nginx / Caddy / Traefik 暴露公网 HTTPS
3. `telegram.webhook_url` 使用公网 HTTPS 地址
4. `telegram.webhook_path` 与反代转发路径保持一致

Nginx 示例：

```nginx
server {
    listen 443 ssl http2;
    server_name your-domain.example;

    ssl_certificate     /path/to/fullchain.pem;
    ssl_certificate_key /path/to/privkey.pem;

    location /webhook {
        proxy_pass http://127.0.0.1:8443/webhook;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

对应配置：

```toml
[telegram]
bot_token = "123456:ABCDEF"
mode = "webhook"
webhook_url = "https://your-domain.example/webhook"
webhook_listen = "127.0.0.1:8443"
webhook_path = "/webhook"
```

## Bot 命令

- `/start`
- `/help`
- `/language <en|zh-CN|zh-TW>`
- `/logs [n]`
- `/log_latest_tail [n]`

## 日志

默认日志目录为 `./logs`，按天轮转。

- `/logs`
  打包全部日志
- `/logs 3`
  只打包最近 3 个日志文件
- `/log_latest_tail`
  默认返回最新日志尾部 100 行
- `/log_latest_tail 200`
  返回最新日志尾部 200 行，并自动截断到 Telegram 安全长度

## 快速开始

1. 从下面三种模板里任选一种作为起点：
   - [config.minimal.toml](/Users/hsuyelin/Documents/Developer/Github/OCI-Sniper-rs/config.minimal.toml)
   - [config.webhook.toml](/Users/hsuyelin/Documents/Developer/Github/OCI-Sniper-rs/config.webhook.toml)
   - [config.example.toml](/Users/hsuyelin/Documents/Developer/Github/OCI-Sniper-rs/config.example.toml)
2. 准备标准 OCI 配置文件和 API 私钥
3. 如需默认实例创建模式，确保存在可用 subnet 和默认 SSH 公钥
4. 执行：

```bash
cargo run -- --lang zh-CN test-api
cargo run -- --lang zh-CN run --dry-run
```

最小 polling 启动流程：

```bash
cp config.minimal.toml config.toml
$EDITOR config.toml
cargo run -- --lang zh-CN test-api
cargo run -- --lang zh-CN run
```

webhook 启动流程：

```bash
cp config.webhook.toml config.toml
$EDITOR config.toml
cargo run -- bot-webhook --set https://your-domain.example/webhook
cargo run -- --lang zh-CN run
```

## 校验

当前仓库使用以下本地校验命令：

```bash
cargo fmt -- --check
cargo check
cargo test
cargo clippy -- -D warnings
```
