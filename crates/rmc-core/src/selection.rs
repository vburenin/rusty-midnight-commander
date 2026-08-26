use std::collections::BTreeSet;

#[derive(Debug, Default, Clone)]
pub struct Selection {
    indices: BTreeSet<usize>,
}

impl Selection {
    pub fn clear(&mut self) {
        self.indices.clear();
    }
    pub fn is_selected(&self, idx: usize) -> bool {
        self.indices.contains(&idx)
    }
    pub fn toggle(&mut self, idx: usize) {
        if !self.indices.insert(idx) {
            self.indices.remove(&idx);
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.indices.iter().copied()
    }
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}
