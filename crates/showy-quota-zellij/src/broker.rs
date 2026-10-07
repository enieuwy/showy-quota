use std::collections::BTreeMap;

/// A broker lease uses plugin messages only. CLI pipe input cannot acquire it.
#[derive(Debug, Default)]
pub struct Broker {
    pub enabled: bool,
    pub id: u32,
    pub group: String,
    pub leader: Option<u32>,
    pub last_seen: Option<f64>,
    pub election_at: f64,
    pub sequence: u64,
    pub received_sequence: u64,
    peers: BTreeMap<u32, f64>,
    timer_pending: bool,
}

impl Broker {
    pub fn start(&mut self, id: u32, group: String, now: f64) {
        self.enabled = true;
        self.id = id;
        self.group = group;
        self.election_at = now + 0.5;
        self.peers.insert(id, now);
    }

    pub fn observe(&mut self, sender: u32, now: f64, owner: bool) {
        if !self.enabled {
            return;
        }
        self.peers.insert(sender, now);
        if owner && self.leader.is_none_or(|leader| sender <= leader) {
            if self.leader != Some(sender) {
                self.received_sequence = 0;
            }
            self.leader = Some(sender);
            self.last_seen = Some(now);
        }
        if self.is_owner() && sender < self.id {
            self.leader = Some(sender);
            self.last_seen = Some(now);
            self.received_sequence = 0;
        }
    }

    pub fn tick(&mut self, now: f64) -> bool {
        if !self.enabled {
            return false;
        }
        self.peers.insert(self.id, now);
        self.peers.retain(|_, seen| (now - *seen).max(0.0) < 10.0);
        if self.leader.is_some_and(|id| id != self.id)
            && self
                .last_seen
                .is_some_and(|seen| (now - seen).max(0.0) >= 10.0)
        {
            if let Some(id) = self.leader.take() {
                self.peers.remove(&id);
            }
            self.last_seen = None;
            self.election_at = now + 0.5;
            return false;
        }
        if self.leader.is_none() && now >= self.election_at {
            self.leader = self.peers.keys().next().copied();
            self.last_seen = Some(now);
            self.received_sequence = 0;
        }
        self.is_owner()
    }

    /// The broker owns one wakeup chain; coordinator schedules only set deadlines.
    pub fn arm_timer(&mut self, now: f64, refresh_at: Option<f64>) -> Option<f64> {
        if !self.enabled || self.timer_pending {
            return None;
        }
        let mut delay = if self.leader.is_none() && now < self.election_at {
            self.election_at - now
        } else {
            3.0
        };
        if self.is_owner() {
            if let Some(deadline) = refresh_at {
                delay = delay.min((deadline - now).max(0.1));
            }
        }
        self.timer_pending = true;
        Some(delay.max(0.1))
    }

    pub fn timer_fired(&mut self) {
        self.timer_pending = false;
    }

    pub fn is_owner(&self) -> bool {
        !self.enabled || self.leader == Some(self.id)
    }
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ten_tabs_choose_one_owner_and_recover_after_owner_loss() {
        let mut tabs: Vec<_> = (10..20)
            .map(|id| {
                let mut broker = Broker::default();
                broker.start(id, "same".into(), 0.0);
                broker
            })
            .collect();
        for tab in &mut tabs {
            for id in 10..20 {
                tab.observe(id, 0.0, false);
            }
            tab.tick(0.6);
        }
        assert_eq!(tabs.iter().filter(|tab| tab.is_owner()).count(), 1);
        for tab in &mut tabs[1..] {
            tab.observe(10, 1.0, true);
            tab.tick(11.1);
        }
        for tab in &mut tabs[1..] {
            for id in 11..20 {
                tab.observe(id, 11.1, false);
            }
            tab.tick(11.7);
        }
        assert_eq!(tabs[1..].iter().filter(|tab| tab.is_owner()).count(), 1);
        assert!(tabs[1].is_owner());
    }
    #[test]
    fn late_lower_owner_stops_duplicate_collection() {
        let mut broker = Broker::default();
        broker.start(20, "same".into(), 0.0);
        assert!(broker.tick(1.0));
        broker.observe(10, 1.0, true);
        assert!(!broker.is_owner());
        assert_eq!(broker.leader, Some(10));
    }

    #[test]
    fn refresh_deadlines_keep_one_broker_wakeup_chain() {
        let mut broker = Broker::default();
        broker.start(10, "same".into(), 0.0);
        assert_eq!(broker.arm_timer(0.0, None), Some(0.5));
        assert_eq!(broker.arm_timer(0.0, None), None);
        let mut now = 0.5;
        broker.timer_fired();
        assert!(broker.tick(now));
        for _ in 0..20 {
            // Every coordinator refresh replaces its deadline, not the wakeup.
            assert_eq!(broker.arm_timer(now, Some(now + 1.0)), Some(1.0));
            assert_eq!(broker.arm_timer(now, Some(now + 1.0)), None);
            now += 1.0;
            broker.timer_fired();
            assert!(broker.tick(now));
        }
        assert_eq!(broker.arm_timer(now, Some(now + 30.0)), Some(3.0));
    }

    #[test]
    fn followers_keep_heartbeats_without_running_owner_deadlines() {
        let mut broker = Broker::default();
        broker.start(20, "same".into(), 0.0);
        broker.observe(10, 0.0, true);
        assert_eq!(broker.arm_timer(0.0, Some(0.1)), Some(3.0));
        assert_eq!(broker.arm_timer(0.0, Some(0.1)), None);
        broker.timer_fired();
        assert!(!broker.tick(3.0));
        assert_eq!(broker.arm_timer(3.0, Some(0.1)), Some(3.0));
    }
}
