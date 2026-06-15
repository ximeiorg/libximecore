# ximed API 文档

ximed 是一个输入法公共服务模块，提供设备配对和剪贴板同步功能。

基础 URL: `http://localhost:5023`

```
                   POST /pair/request {device_name}
                         │
                 服务端生成 device_id (UUID)
                         │
              ┌──────────┴──────────┐
              │ 返回 {device_id,    │
              │       code,         │
              │       expires_in}   │
              └─────────────────────┘
                         │
        POST /pair/confirm {code, approve, device_name}
                         │
              ┌──────────┴──────────┐
              │ 双方生成 token      │
              │ 创建 Pairing 关系   │
              │ 返回 {device_id,    │
              │       token}        │
              └─────────────────────┘
                         │
         GET /pair/list  (Bearer Token)
              ┌──────────┴──────────┐
              │ 解码 token →        │
              │ device_id → 查配对  │
              └─────────────────────
```

---

## 接口一览

| 方法 | 路径 | 说明 | 需要认证 |
|------|------|------|----------|
| GET | `/health` | 健康检查 | 否 |
| POST | `/pair/request` | 发起配对请求 | 否 |
| GET | `/pair/status` | 查询配对状态 | 否 |
| POST | `/pair/confirm` | 确认/拒绝配对 | 否 |
| GET | `/pair/list` | 列出与当前设备配对的设备 | **是**(Bearer Token) |
| POST | `/pair/remove/{device_id}` | 移除已配对设备 | **是**(Bearer Token) |
| GET | `/clipboard/read` | 读取剪贴板 | **是**(Bearer Token) |
| POST | `/clipboard/write` | 写入剪贴板 | **是**(Bearer Token) |

---

## 1. 健康检查

**用途**：检测服务是否正常运行。

```
GET /health
```

**示例响应 (200)**:
```json
{
  "status": "ok",
  "timestamp": "2026-05-23T13:40:00+00:00"
}
```

---

## 2. 配对管理

配对流程为两阶段：
1. 设备 A 调用 `/pair/request` 获取配对码
2. 设备 B 调用 `/pair/confirm` 确认该配对码
3. 设备 A 轮询 `/pair/status` 获取认证 Token

### 2.1 发起配对请求

**用途**：设备发起配对请求，生成 6 位数字配对码，有效期 10 分钟。

```
POST /pair/request
```

**请求体**:
| 字段 | 类型 | 说明 |
|------|------|------|
| device_name | string | 设备展示名称 |

**示例请求**:
```json
{
  "device_name": "My Phone"
}
```

**示例响应 (200)**:
```json
{
  "device_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "code": "482931",
  "expires_in": 600
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| device_id | string | 服务端生成的设备唯一标识 (UUID) |
| code | string | 6 位数字配对码 |
| expires_in | u64 | 过期时间（秒） |

**错误响应**:
- `400 Bad Request` — device_name 为空

---

### 2.2 查询配对状态

**用途**：轮询配对码的当前状态，当配对被确认后可获取认证 Token。

```
GET /pair/status?code=482931
```

**查询参数**:
| 参数 | 类型 | 说明 |
|------|------|------|
| code | string | 配对码 |

**示例响应 (200 - Pending)**:
```json
{
  "status": "Pending",
  "device_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "token": null,
  "expires_in": 485
}
```

**示例响应 (200 - Confirmed)**:
```json
{
  "status": "Confirmed",
  "device_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "token": "YjJjM2Q0ZTUtZjZhNy04OTAxLWJjZGUtZjEyMzQ1Njc4OTAx.abc123signature",
  "expires_in": 604800
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| status | string | 配对状态：`Pending` / `Confirmed` / `Rejected` / `Expired` |
| device_id | string\|null | 请求方的设备 ID（服务端生成） |
| token | string\|null | Bearer 认证 Token，仅在 Confirmed 时有值，有效期 7 天 |
| expires_in | u64 | 剩余有效时间（秒） |

**错误响应**:
- `404 Not Found` — 配对码不存在
- `404 Not Found` (PairCodeExpired) — 配对码已过期

---

### 2.3 确认/拒绝配对

**用途**：在另一台设备上确认或拒绝某个配对码。

```
POST /pair/confirm
```

**请求体**:
| 字段 | 类型 | 说明 |
|------|------|------|
| code | string | 配对码 |
| approve | boolean | `true` 确认配对 / `false` 拒绝配对 |
| device_name | string | 确认方的设备展示名称 |

**示例请求**:
```json
{
  "code": "482931",
  "approve": true,
  "device_name": "My Desktop"
}
```

**示例响应 (200 - 确认)**:
```json
{
  "success": true,
  "device_id": "b2c3d4e5-f6a7-8901-bcde-f12345678901",
  "token": "YjJjM2Q0ZTUtZjZhNy04OTAxLWJjZGUtZjEyMzQ1Njc4OTAx.abc123signature"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| success | bool | 操作是否成功 |
| device_id | string | 服务端为确认方生成的设备 ID |
| token | string | 确认方的 Bearer 认证 Token，有效期 7 天 |

**错误响应**:
- `404 Not Found` — 配对码不存在
- `404 Not Found` (PairCodeExpired) — 配对码已过期
- `409 Conflict` — 配对码已被确认

---

### 2.4 列出已配对设备

**用途**：获取与当前认证设备配对的所有设备。需要 Bearer Token 认证，从 token 中提取设备身份。

```
GET /pair/list
Authorization: Bearer <token>
```

**请求头**:
| 头 | 值 |
|----|-----|
| Authorization | `Bearer your-auth-token` |

**示例响应 (200)**:
```json
{
  "devices": [
    {
      "device_id": "b2c3d4e5-f6a7-8901-bcde-f12345678901",
      "device_name": "My Desktop",
      "paired_at": "2026-05-20T10:30:00+00:00",
      "last_seen": "2026-05-23T12:00:00+00:00"
    }
  ]
}
```

**错误响应**:
- `401 Unauthorized` — Token 无效或过期

---

### 2.5 移除已配对设备

**用途**：将指定设备从配对列表中移除。需要 Bearer Token 认证。

```
POST /pair/remove/{device_id}
Authorization: Bearer <token>
```

**请求头**:
| 头 | 值 |
|----|-----|
| Authorization | `Bearer your-auth-token` |

**路径参数**:
| 参数 | 类型 | 说明 |
|------|------|------|
| device_id | string | 要移除的设备唯一标识 |

**示例响应 (200)**:
```json
{
  "removed": true
}
```

**错误响应**:
- `401 Unauthorized` — Token 无效或过期

---

## 3. 剪贴板同步

> 所有剪贴板接口**需要 Bearer Token 认证**，Token 从配对流程获取。

### 3.1 读取剪贴板

**用途**：读取当前系统剪贴板内容。支持增量查询（通过 `since_hash` 参数），当内容未变化时返回空内容以节省带宽。

```
GET /clipboard/read?since_hash=
```

**请求头**:
| 头 | 值 |
|----|-----|
| Authorization | `Bearer your-auth-token` |

**查询参数**:
| 参数 | 类型 | 说明 |
|------|------|------|
| since_hash | string (可选) | 上次已知内容的 SHA-256 hash。如果传此参数且内容未变，返回空 content |

**示例响应 (200 - 内容变化或首次读取)**:
```json
{
  "content": "Hello World",
  "hash": "uU0nuZNNPgilLlLX2Q2Z2OLU_lKlovDvLwN49s_HiRo"
}
```

**示例响应 (200 - 内容未变化)**:
```json
{
  "content": "",
  "hash": "uU0nuZNNPgilLlLX2Q2Z2OLU_lKlovDvLwN49s_HiRo"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| content | string | 剪贴板文本内容；未变化时返回空字符串 |
| hash | string | 剪贴板内容的 SHA-256 Base64 URL-Safe 哈希 |

**错误响应**:
- `401 Unauthorized` — Token 无效或过期
- `500 Internal Server Error` — 剪贴板读取失败

---

### 3.2 写入剪贴板

**用途**：向系统剪贴板写入新内容。先验证 hash 一致性，再检查内容是否已是最新，避免重复写入。

```
POST /clipboard/write
```

**请求头**:
| 头 | 值 |
|----|-----|
| Authorization | `Bearer your-auth-token` |
| Content-Type | application/json |

**请求体**:
| 字段 | 类型 | 说明 |
|------|------|------|
| content | string | 要写入的文本内容 |
| hash | string | content 的 SHA-256 Base64 URL-Safe 哈希 |

**示例请求**:
```json
{
  "content": "Hello from paired device!",
  "hash": "uU0nuZNNPgilLlLX2Q2Z2OLU_lKlovDvLwN49s_HiRo"
}
```

**示例响应 (200 - 写入成功)**:
```json
{
  "accepted": true,
  "hash": "uU0nuZNNPgilLlLX2Q2Z2OLU_lKlovDvLwN49s_HiRo"
}
```

**示例响应 (200 - 内容已是最新，跳过写入)**:
```json
{
  "accepted": false,
  "hash": "uU0nuZNNPgilLlLX2Q2Z2OLU_lKlovDvLwN49s_HiRo"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| accepted | bool | true=已写入剪贴板；false=内容已是最新，跳过 |
| hash | string | 实际写入（或当前）内容的 SHA-256 hash |

**错误响应**:
- `400 Bad Request` (HashMismatch) — 提供的 hash 与 content 不匹配
- `401 Unauthorized` — Token 无效或过期
- `500 Internal Server Error` — 剪贴板读写失败

---

## 4. 认证机制

### Token 格式
```
Base64URL(device_id).Base64URL(HMAC-SHA256(device_id:expires_timestamp))
```

### Token 生命周期
- 有效期：7 天
- 生成时机：配对被另一台设备确认时
- 验证方式：`Authorization: Bearer <token>`

### 公共 API（无需认证）
- `/health`
- `/pair/*` — 配对流程所有接口
