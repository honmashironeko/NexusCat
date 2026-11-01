use nc_core::{AppNotification, NotificationAction, NotificationCategory, Severity};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_NOTIFICATIONS: usize = 200;

#[derive(Debug, Default)]
pub struct NotificationCenter {
    next_id: u64,
    items: Vec<AppNotification>,
}

impl NotificationCenter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(
        &mut self,
        severity: Severity,
        category: NotificationCategory,
        title: impl Into<String>,
        body: impl Into<String>,
        action: Option<NotificationAction>,
    ) -> AppNotification {
        self.next_id = self.next_id.saturating_add(1);
        let notification = AppNotification {
            id: self.next_id,
            severity,
            category,
            title: title.into(),
            body: body.into(),
            action,
            timestamp_ms: now_ms(),
            read: false,
        };
        self.items.push(notification.clone());
        if self.items.len() > MAX_NOTIFICATIONS {
            let excess = self.items.len() - MAX_NOTIFICATIONS;
            self.items.drain(..excess);
        }
        notification
    }

    pub fn list(&self) -> Vec<AppNotification> {
        let mut items = self.items.clone();
        items.reverse();
        items
    }

    pub fn unread_count(&self) -> usize {
        self.items.iter().filter(|item| !item.read).count()
    }

    pub fn latest_unread(&self) -> Option<AppNotification> {
        self.items.iter().rev().find(|item| !item.read).cloned()
    }

    pub fn mark_read(&mut self, id: u64) -> bool {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.read = true;
            true
        } else {
            false
        }
    }

    pub fn mark_all_read(&mut self) {
        for item in &mut self.items {
            item.read = true;
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
