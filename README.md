# autojiedian

`autojiedian` 是一个自建的 Clash / Mihomo 订阅聚合、筛选和可控分发仓库。

它当前承担三件事：

- 汇总多路公益与镜像订阅源，生成新的 `clash.yaml`
- 保留 `artifacts` 与 `source-registry`，方便排查来源质量
- 通过 GitHub Pages 提供稳定的分发落点，作为 `raw` 之外的可控兜底层

## 分发地址

建议的拉取顺序：


GitHub Pages 还会同步发布：

- `summary.json`
- `source-registry.json`
- `validated_pool.json`
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

编辑 `conf/config.toml`，补充或替换你要聚合的订阅源。

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
- `validated_pool.json` 是面向自动任务消费的候选池产物
- `artifacts/` 存放每轮聚合过程中的中间结果
- `rules/` 存放仓库自托管的规则文件
- GitHub Pages 是对外稳定分发层，不依赖上游仓库页面

## 维护说明

- 当前仓库已切换到自有分发地址与自有规则地址
- 若未来要进一步“脱 fork”，那是 GitHub 仓库关系层面的动作，需要单独处理；代码、文档、工作流层面已经可以独立维护
