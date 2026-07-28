use rustc_hash::FxHashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pair {
    pub a_id: usize,
    pub b_id: usize,
}

#[derive(Debug, Default)]
pub struct Map {
    a_to_b: FxHashMap<usize, usize>,
    b_to_a: FxHashMap<usize, usize>,
}

impl Map {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_match(&mut self, a_id: usize, b_id: usize) {
        self.a_to_b.insert(a_id, b_id);
        self.b_to_a.insert(b_id, a_id);
    }

    #[must_use]
    pub fn get_match_a_to_b(&self, a_id: usize) -> Option<usize> {
        self.a_to_b.get(&a_id).copied()
    }

    #[must_use]
    pub fn get_match_b_to_a(&self, b_id: usize) -> Option<usize> {
        self.b_to_a.get(&b_id).copied()
    }

    #[must_use]
    pub fn is_matched_a(&self, a_id: usize) -> bool {
        self.a_to_b.contains_key(&a_id)
    }

    #[must_use]
    pub fn is_matched_b(&self, b_id: usize) -> bool {
        self.b_to_a.contains_key(&b_id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.a_to_b.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.a_to_b.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = Pair> + '_ {
        self.a_to_b.iter().map(|(&a_id, &b_id)| Pair { a_id, b_id })
    }
}
