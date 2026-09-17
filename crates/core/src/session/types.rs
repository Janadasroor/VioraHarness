// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub model: String,
    pub created_at: i64,
}
