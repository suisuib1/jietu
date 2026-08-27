use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PinKind {
    Image,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImagePinPayload {
    pub(crate) png: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PinRecord {
    pub(crate) pin_id: String,
    pub(crate) kind: PinKind,
    pub(crate) window_label: String,
    pub(crate) payload: ImagePinPayload,
    pub(crate) created_at_ms: i64,
    pub(crate) locked: bool,
    pub(crate) opacity: f64,
}

#[derive(Default)]
pub(crate) struct PinManager {
    next_id: u64,
    pins: BTreeMap<String, PinRecord>,
}

impl PinManager {
    pub(crate) fn create_image(&mut self, payload: ImagePinPayload) -> String {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let pin_id = self.next_id.to_string();
        self.pins.insert(
            pin_id.clone(),
            PinRecord {
                pin_id: pin_id.clone(),
                kind: PinKind::Image,
                window_label: format!("pin-image-{pin_id}"),
                payload,
                created_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .min(i64::MAX as u128) as i64,
                locked: false,
                opacity: 1.0,
            },
        );
        pin_id
    }
    pub(crate) fn get(&self, pin_id: &str) -> Option<&PinRecord> {
        self.pins.get(pin_id)
    }
    pub(crate) fn get_mut(&mut self, pin_id: &str) -> Option<&mut PinRecord> {
        self.pins.get_mut(pin_id)
    }
    pub(crate) fn remove(&mut self, pin_id: &str) -> Option<PinRecord> {
        self.pins.remove(pin_id)
    }
    pub(crate) fn list_active_pins(&self) -> impl Iterator<Item = &PinRecord> {
        self.pins.values()
    }
    pub(crate) fn close_all(&mut self) -> Vec<PinRecord> {
        std::mem::take(&mut self.pins).into_values().collect()
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        self.pins.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn payload(value: u8) -> ImagePinPayload {
        ImagePinPayload {
            png: vec![value],
            width: 2,
            height: 3,
        }
    }
    #[test]
    fn creates_unique_image_pins_and_labels() {
        let mut manager = PinManager::default();
        let first = manager.create_image(payload(1));
        let second = manager.create_image(payload(2));
        assert_ne!(first, second);
        assert_eq!(manager.get(&first).unwrap().window_label, "pin-image-1");
        assert_eq!(manager.get(&second).unwrap().window_label, "pin-image-2");
        assert_eq!(manager.len(), 2);
    }
    #[test]
    fn removing_one_pin_keeps_the_others() {
        let mut manager = PinManager::default();
        let first = manager.create_image(payload(1));
        let second = manager.create_image(payload(2));
        assert_eq!(manager.remove(&first).unwrap().payload, payload(1));
        assert!(manager.get(&first).is_none());
        assert_eq!(manager.get(&second).unwrap().payload, payload(2));
    }
    #[test]
    fn close_all_releases_every_payload() {
        let mut manager = PinManager::default();
        manager.create_image(payload(1));
        manager.create_image(payload(2));
        assert_eq!(manager.close_all().len(), 2);
        assert_eq!(manager.len(), 0);
    }
    #[test]
    fn opacity_is_independent_and_lock_is_mutable() {
        let mut manager = PinManager::default();
        let id = manager.create_image(payload(1));
        let pin = manager.get_mut(&id).unwrap();
        pin.opacity = 0.4;
        pin.locked = true;
        assert_eq!(manager.get(&id).unwrap().opacity, 0.4);
        assert!(manager.get(&id).unwrap().locked);
    }
}
