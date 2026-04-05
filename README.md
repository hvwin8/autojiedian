# autojiedian

`autojiedian` 是一个自建的 Clash / Mihomo 订阅聚合、筛选和可控分发仓库。

它当前承担三件事：

- 汇总多路公益与镜像订阅源，生成新的 `clash.yaml`
- 保留 `artifacts` 与 `source-registry`，方便排查来源质量
- 通过 GitHub Pages 提供稳定的分发落点，作为 `raw` 之外的可控兜底层

当前默认采用“分层采集 -> 统一验证 -> 最终分发”的保守策略：

- 第一层：直接拉取稳定、长期更新的 raw YAML / Meta 产物
- 第二层：扫描 GitHub tree / README 这类发现页，补充会变动的候选源
- 第三层：只把通过连通性和能力验证的节点写入最终发布地址

## 分发地址

建议的拉取顺序：

- Clash / Mihomo 主订阅：
  - `https://raw.githubusercontent.com/hvwin8/autojiedian/master/clash.yaml`
  - `https://hvwin8.github.io/autojiedian/clash.yaml`
- v2rayN Base64 订阅：
  - `https://raw.githubusercontent.com/hvwin8/autojiedian/master/v2rayn.txt`
  - `https://hvwin8.github.io/autojiedian/v2rayn.txt`
- v2rayN 节点直出列表：
  - `https://raw.githubusercontent.com/hvwin8/autojiedian/master/v2rayn-links.txt`
  - `https://hvwin8.github.io/autojiedian/v2rayn-links.txt`

GitHub Pages 还会同步发布：

- `summary.json`
- `source-registry.json`
- `validated_pool.json`
- `validated_pool_mihomo.json`
- `v2rayn.txt`
- `v2rayn-links.txt`
- `latest.json`
- `rules/`

项目页入口：

- `https://hvwin8.github.io/autojiedian/`

## 工作流

仓库当前分成三条独立链路：

- `ci`
  - 负责格式检查、静态检查、构建验证
  - 面向 `push` / `pull_request` / 手动触发
- `refresh-release`
  - 负责定时执行 `cargo run`，刷新 `clash.yaml` 与 `artifacts`
  - 仅手动或定时触发，不再跟每次推送绑死
- `pages`
  - 负责把最新提交内容打包成 GitHub Pages 站点

这样的拆分可以避免每次普通提交都跑重型刷新任务，同时保留自动产物更新能力。

## 本地使用

### 1. 修改源配置

编辑 `conf/config.toml`，按分层思路补充或替换你要聚合的订阅源。

- `subs`
  - 第一层直连主源
  - 只放值得长期保留的稳定 raw 输出
- `discover_feeds`
  - 第二层发现页
  - 用来补充可能新增、迁移或分裂出来的公开输出
- `pools`
  - 预留给额外候选池输入
  - 默认关闭，避免把高噪声来源直接抬进主链路

### 2. 生成产物

```powershell
cargo run
```

运行后会更新：

- `clash.yaml`
- `artifacts/*.json`

### 3. 本地构建 Pages 产物

```powershell
python scripts/build_pages.py --output-dir _site --base-url https://hvwin8.github.io/autojiedian
```

## 仓库约定

- `clash.yaml` 是当前对外发布的主产物
- `v2rayn.txt` 是面向 `v2rayN` 的 Base64 订阅产物
- `v2rayn-links.txt` 是逐行节点链接产物，方便直接查看或手动导入
- `validated_pool.json` 是面向自动任务消费的候选池产物
- `validated_pool_mihomo.json` 是面向 Mihomo 编排的增强候选池产物，额外带出口 IP、地区提示和能力标记
- `artifacts/` 存放每轮聚合过程中的中间结果
- `rules/` 存放仓库自托管的规则文件
- GitHub Pages 是对外稳定分发层，不依赖上游仓库页面

## 维护说明

- 当前仓库已切换到自有分发地址与自有规则地址
- 若未来要进一步“脱 fork”，那是 GitHub 仓库关系层面的动作，需要单独处理；代码、文档、工作流层面已经可以独立维护
