use super::store::SessionStore;

pub struct Projector<'a> {
    pub store: &'a SessionStore,
}

impl<'a> Projector<'a> {
    pub fn new(store: &'a SessionStore) -> Self {
        Self { store }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_over_mem_store() {
        let store = SessionStore::new_in_memory().unwrap();
        let p = Projector::new(&store);
        assert!(p.store.count_sessions(false).unwrap() >= 0);
    }
}
