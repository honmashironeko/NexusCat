const CANDIDATE_TICKS: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragObservation {
    pub button_down: bool,
    pub change_count: Option<i64>,
    pub has_files: bool,
}

#[derive(Debug)]
pub struct DragSessionTracker {
    last_change: Option<i64>,
    candidate: Option<(i64, u8)>,
    active: bool,
}

impl DragSessionTracker {
    pub fn new(change_count: Option<i64>) -> Self {
        Self {
            last_change: change_count,
            candidate: None,
            active: false,
        }
    }

    pub fn update(&mut self, observation: DragObservation) -> bool {
        let Some(change_count) = observation.change_count else {
            self.active = false;
            self.candidate = None;
            return false;
        };
        if !observation.button_down {
            self.active = false;
            self.candidate = None;
            self.last_change = Some(change_count);
            return false;
        }
        if self.active {
            if self.last_change != Some(change_count) {
                self.active = false;
                self.last_change = Some(change_count);
            }
            return self.active;
        }
        if self.last_change == Some(change_count) {
            self.candidate = None;
            return false;
        }
        if observation.has_files {
            self.active = true;
            self.candidate = None;
            self.last_change = Some(change_count);
            return true;
        }
        if let Some((candidate_change, ticks)) = self.candidate {
            if candidate_change == change_count {
                let ticks = ticks.saturating_add(1);
                if ticks >= CANDIDATE_TICKS {
                    self.candidate = None;
                    self.last_change = Some(change_count);
                } else {
                    self.candidate = Some((change_count, ticks));
                }
                return false;
            }
        }
        self.candidate = Some((change_count, 1));
        false
    }
}

