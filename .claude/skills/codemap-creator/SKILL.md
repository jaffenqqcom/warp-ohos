---
name: codemap-creator
description: 这是一个专用的write-skill的skill，只用于创建代和更新项目的代码地图的skill。目的是创建一个新的skill，这个新的skill里记录了项目的代码地图，方便agent根据代码地图快速掌握代码的情况，减少摸索时间。

方法是调用writting-skills，给writting-skills投喂正确的描述信息，让writting-skills可以创建出满足要求的代码地图skill

# Code Map Creator

## 概述

调用writting-skills，给writting-skills投喂正确的描述信息，让writting-skills可以创建出满足要求的代码地图skill

## 投喂给writting-skills的内容包括

1、 要创建的code map skill的名称，改名称有用户在触发code-map-creator时输出

2、 code map skill要有快速参考

- **模块条目** — 路径、职责、关键文件、接口、依赖
- **执行路径条目** — 触发条件、调用链、关键接口
- **接口契约条目** — 签名、调用方、被调方、数据流


3、 code map skii的核心模板

3.1 模块条目

只跟踪跨模块的公开接口。内部实现细节是噪音。

````markdown
## 模块: <模块名>

**路径**: `path/to/module/`

**职责**: 一句话描述用途。

**关键文件**:
- 入口/导出，比如“entry.rs”
- 核心类型, 比如“types.rs”
- 对外接口，比如`handler.rs`

**对外接口**:
| 接口 | 定义位置 | 调用方 |
|------|----------|--------|
| `fn init()` | `init.rs:45` | `main.rs:120` |

**依赖模块**: `other_module` (通过 trait), `base_module` (通过 `new()`)
````

3.2 执行路径条目

记录每条路径的触发条件和关键调用链。在模块边界处停止。

````markdown
## 路径: <路径名>

**功能**: 这条路径做什么。

**触发条件**: 事件、启动、用户操作。

**线路**:
```
开始 → mod_a::init()    [path/a.rs:45]
     → mod_b::setup()   [path/b.rs:30]
     → mod_c::run()     [path/c.rs:12]
```
````

3.3 接口契约条目

只记录跨模块边界的接口。

````markdown
## 接口: `trait Handler`

**定义**: `path/to/file.rs:10`

**签名**: `fn handle(input: Type) -> Result<Output>`

**调用方**: `caller::run()` at `caller.rs:50`

**被调者**: `impl::handle()` at `impl.rs:80`
````

3.4 构建地图的步骤

 **找入口点**：`main()`、`init()`、事件处理器、插件注册
 **追踪每条路径**：向外跟踪调用。到工具函数就停（那是实现细节）
 **记录接口**：只记跨模块边界的 trait 和 `pub fn`
 **标注数据流**：什么进来、什么出去、在哪里转换
 **写入 memory**：`memory/code-map.md` + 在 `MEMORY.md` 加指针

3.5常见错误

- **列出每个文件**：只需要入口点、边界接口、关键类型
- **遗漏触发条件**：没有触发条件的路径没用——你永远不知道它什么时候跑
- **地图过期不更新**：改了一个模块后更新条目。过期的地图比没有还误导
- **追得太深**：三层就够了。再深就是实现细节
