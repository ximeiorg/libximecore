use crate::error::ApiError;
use crate::types::PairStatus;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

const PAIR_CODE_VALIDITY_MINUTES: i64 = 10;
const PAIRS_FILE: &str = "pairs.json";

/// 已配对设备的信息（每个设备一条记录，带有自己的 token）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: String,
    pub device_name: String,
    pub token: String,
    pub paired_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// 两个设备之间的配对关系（多对多）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pairing {
    pub pairing_id: String,
    /// 设备 A 的 device_id
    pub device_a: String,
    /// 设备 B 的 device_id
    pub device_b: String,
    pub paired_at: DateTime<Utc>,
}

impl Pairing {
    pub fn new(device_a: String, device_b: String) -> Self {
        Self {
            pairing_id: Uuid::new_v4().to_string(),
            device_a,
            device_b,
            paired_at: Utc::now(),
        }
    }

    /// 检查指定设备是否参与此配对
    pub fn involves_device(&self, device_id: &str) -> bool {
        self.device_a == device_id || self.device_b == device_id
    }

    /// 获取配对中的另一方设备 ID
    pub fn peer_of(&self, device_id: &str) -> Option<&str> {
        if self.device_a == device_id {
            Some(&self.device_b)
        } else if self.device_b == device_id {
            Some(&self.device_a)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairSession {
    pub code: String,
    /// 发起配对请求的设备 ID
    pub requester_id: String,
    /// 发起配对请求的设备名称
    pub requester_name: String,
    /// 确认配对的设备 ID（确认后设置）
    pub confirmer_id: Option<String>,
    /// 确认配对的设备名称（确认后设置）
    pub confirmer_name: Option<String>,
    /// 请求方 token（确认后设置，供 pair_status 轮询获取）
    pub requester_token: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: PairStatus,
}

impl PairSession {
    pub fn new(requester_id: String, requester_name: String) -> Self {
        let now = Utc::now();
        let code = generate_pair_code();
        Self {
            code,
            requester_id,
            requester_name,
            confirmer_id: None,
            confirmer_name: None,
            requester_token: None,
            created_at: now,
            expires_at: now + Duration::minutes(PAIR_CODE_VALIDITY_MINUTES),
            status: PairStatus::Pending,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at < Utc::now()
    }

    pub fn expires_in_seconds(&self) -> u64 {
        let now = Utc::now();
        if self.expires_at > now {
            (self.expires_at - now).num_seconds() as u64
        } else {
            0
        }
    }
}

fn generate_pair_code() -> String {
    let code = Uuid::new_v4();
    let bytes = code.as_bytes();
    let num = ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | (bytes[2] as u32);
    format!("{:06}", num % 1_000_000)
}

/// 存储到 JSON 文件的顶层结构
#[derive(Debug, Serialize, Deserialize)]
struct StoreData {
    devices: Vec<PairedDevice>,
    pairings: Vec<Pairing>,
}

#[derive(Debug)]
pub struct PairStore {
    config_dir: PathBuf,
    /// 所有已配对的设备，keyed by device_id
    devices: HashMap<String, PairedDevice>,
    /// 所有配对关系
    pairings: Vec<Pairing>,
    pub pending_sessions: HashMap<String, PairSession>,
}

impl PairStore {
    pub fn load_from(config_dir: PathBuf) -> Self {
        let path = config_dir.join(PAIRS_FILE);
        let (devices, pairings) = if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(data) = serde_json::from_str::<StoreData>(&content) {
                    let dev_map = data
                        .devices
                        .into_iter()
                        .map(|d| (d.device_id.clone(), d))
                        .collect();
                    (dev_map, data.pairings)
                } else {
                    // 兼容旧格式：纯设备列表
                    if let Ok(devices) = serde_json::from_str::<Vec<PairedDevice>>(&content) {
                        let dev_map = devices
                            .into_iter()
                            .map(|d| (d.device_id.clone(), d))
                            .collect();
                        (dev_map, Vec::new())
                    } else {
                        (HashMap::new(), Vec::new())
                    }
                }
            } else {
                (HashMap::new(), Vec::new())
            }
        } else {
            (HashMap::new(), Vec::new())
        };

        Self {
            config_dir,
            devices,
            pairings,
            pending_sessions: HashMap::new(),
        }
    }

    pub fn save(&self) -> Result<(), ApiError> {
        let path = self.config_dir.join(PAIRS_FILE);
        if !self.config_dir.exists() {
            fs::create_dir_all(&self.config_dir)?;
        }

        let devices: Vec<PairedDevice> = self.devices.values().cloned().collect();
        let data = StoreData {
            devices,
            pairings: self.pairings.clone(),
        };
        let content = serde_json::to_string_pretty(&data)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// 创建配对会话
    pub fn create_session(&mut self, device_id: String, device_name: String) -> PairSession {
        let session = PairSession::new(device_id, device_name);
        self.pending_sessions
            .insert(session.code.clone(), session.clone());
        session
    }

    pub fn get_session(&self, code: &str) -> Option<&PairSession> {
        self.pending_sessions.get(code)
    }

    pub fn get_session_mut(&mut self, code: &str) -> Option<&mut PairSession> {
        self.pending_sessions.get_mut(code)
    }

    /// 确认配对：为双方生成 token、创建配对关系
    /// 返回 (请求方设备, 确认方设备)
    pub fn confirm_session(
        &mut self,
        code: &str,
        requester_token: String,
        confirmer_id: &str,
        confirmer_name: &str,
        confirmer_token: String,
    ) -> Result<(PairedDevice, PairedDevice), ApiError> {
        let session = self
            .pending_sessions
            .get_mut(code)
            .ok_or(ApiError::PairCodeNotFound)?;

        if session.is_expired() {
            return Err(ApiError::PairCodeExpired);
        }

        if session.status != PairStatus::Pending {
            return Err(ApiError::PairAlreadyConfirmed);
        }

        if confirmer_id == session.requester_id {
            return Err(ApiError::InvalidRequest("cannot pair with yourself".into()));
        }

        session.status = PairStatus::Confirmed;
        session.confirmer_id = Some(confirmer_id.to_string());
        session.confirmer_name = Some(confirmer_name.to_string());

        // 创建或更新请求方设备
        let requester_device = PairedDevice {
            device_id: session.requester_id.clone(),
            device_name: session.requester_name.clone(),
            token: requester_token,
            paired_at: Utc::now(),
            last_seen: Utc::now(),
        };

        // 创建或更新确认方设备
        let confirmer_device = PairedDevice {
            device_id: confirmer_id.to_string(),
            device_name: confirmer_name.to_string(),
            token: confirmer_token,
            paired_at: Utc::now(),
            last_seen: Utc::now(),
        };

        // 检查是否已存在配对关系（防止重复配对）
        let already_paired = self.pairings.iter().any(|p| {
            (p.device_a == requester_device.device_id && p.device_b == confirmer_device.device_id)
                || (p.device_a == confirmer_device.device_id
                    && p.device_b == requester_device.device_id)
        });

        if !already_paired {
            let pairing = Pairing::new(
                requester_device.device_id.clone(),
                confirmer_device.device_id.clone(),
            );
            self.pairings.push(pairing);
        }

        // 更新或插入设备记录（保留已有的 token/paired_at 如果设备已存在）
        if !self.devices.contains_key(&requester_device.device_id) {
            self.devices
                .insert(requester_device.device_id.clone(), requester_device.clone());
        }
        if !self.devices.contains_key(&confirmer_device.device_id) {
            self.devices
                .insert(confirmer_device.device_id.clone(), confirmer_device.clone());
        }

        // 在 session 中保存请求方的 token，供 pair_status 轮询获取
        session.requester_token = Some(requester_device.token.clone());

        self.save()?;

        Ok((requester_device, confirmer_device))
    }

    pub fn reject_session(&mut self, code: &str) -> Result<(), ApiError> {
        let session = self
            .pending_sessions
            .get_mut(code)
            .ok_or(ApiError::PairCodeNotFound)?;

        session.status = PairStatus::Rejected;
        self.pending_sessions.remove(code);
        Ok(())
    }

    pub fn get_device(&self, device_id: &str) -> Option<&PairedDevice> {
        self.devices.get(device_id)
    }

    pub fn get_device_by_token(&self, token: &str) -> Option<&PairedDevice> {
        self.devices.values().find(|d| d.token == token)
    }

    pub fn update_last_seen(&mut self, device_id: &str) {
        if let Some(device) = self.devices.get_mut(device_id) {
            device.last_seen = Utc::now();
            self.save().ok();
        }
    }

    /// 获取与指定设备配对的设备列表
    pub fn get_paired_devices(&self, device_id: &str) -> Vec<&PairedDevice> {
        self.pairings
            .iter()
            .filter(|p| p.involves_device(device_id))
            .filter_map(|p| {
                let peer_id = p.peer_of(device_id)?;
                self.devices.get(peer_id)
            })
            .collect()
    }

    /// 获取所有已配对的设备（不区分配对关系）
    pub fn list_all_devices(&self) -> Vec<&PairedDevice> {
        self.devices.values().collect()
    }

    /// 移除设备及其所有配对关系
    pub fn remove_device(&mut self, device_id: &str) -> Result<(), ApiError> {
        self.devices.remove(device_id);
        self.pairings.retain(|p| !p.involves_device(device_id));
        self.save()?;
        Ok(())
    }

    /// 检查设备是否有任何配对关系
    pub fn has_any_pairing(&self, device_id: &str) -> bool {
        self.pairings.iter().any(|p| p.involves_device(device_id))
    }

    /// 检查两个设备是否已配对
    pub fn are_devices_paired(&self, device_a: &str, device_b: &str) -> bool {
        self.pairings.iter().any(|p| {
            (p.device_a == device_a && p.device_b == device_b)
                || (p.device_a == device_b && p.device_b == device_a)
        })
    }
}
