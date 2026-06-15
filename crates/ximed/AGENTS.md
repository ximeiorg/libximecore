# 一个输入法的公共服务模块

## 项目简介
这是一个输入法的公共服务模块，采用 Rust + TSF 构建。主要用于 windows 和 linux 输入法输入。

## 快速开始
- 开发构建： `cargo build --quiet` (构建)

## 硬性规则（必须遵守，CI 会验证）
- 所有命令使用 powershell
- 修改完必须使用 `cargo build --quiet` 检查是否有错误
- 代码中禁止出现`unwarp()` 和 `expect()` 方法
- 禁止自己运行程序，禁止自己安装
- librime 目录只读，禁止修改
- 禁止提交代码 
- api 文档使用 bruno 记录

## 工作规则
- 每次只做一个功能点
- 当前功能点端到端验证通过后，才能开始下一个
- 不要在实现功能 A 时"顺便"重构功能 B
- 当觉得有必要时，就添加单元测试


## 每次会话开始时（上班打卡）
1. 读 PROGRESS.md 了解当前状态
2. 读 DECISIONS.md 了解重要决策
3. 跑 `./cargo build --quiet` 确认仓库处于一致状态
4. 从 PROGRESS.md 的"下一步"部分继续工作

## 每次会话结束前（下班打卡）
1. 更新 PROGRESS.md
2. 跑 `./cargo build --quiet` 确认一致状态
3. 提交所有已完成的工作
