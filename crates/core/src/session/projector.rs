use super::store::SessionStore;

pub struct Projector<'a> {
    pub store: &'a SessionStore,
}

impl<'a> Projector<'a> {
    pub fn new(store: &'a SessionStore) -> Self {
        Self { store }
    }
}
