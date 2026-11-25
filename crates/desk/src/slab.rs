//! Generational slab: stable small keys, stale keys never alias new items.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Key {
    pub idx: u32,
    pub gen: u32,
}

struct Slot<T> {
    gen: u32,
    val: Option<T>,
}

pub struct Slab<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Default for Slab<T> {
    fn default() -> Self {
        Self { slots: Vec::new(), free: Vec::new(), len: 0 }
    }
}

impl<T> Slab<T> {
    pub fn insert(&mut self, v: T) -> Key {
        self.len += 1;
        if let Some(idx) = self.free.pop() {
            let s = &mut self.slots[idx as usize];
            s.gen = s.gen.wrapping_add(1);
            s.val = Some(v);
            Key { idx, gen: s.gen }
        } else {
            self.slots.push(Slot { gen: 1, val: Some(v) });
            Key { idx: self.slots.len() as u32 - 1, gen: 1 }
        }
    }
    pub fn remove(&mut self, k: Key) -> Option<T> {
        let s = self.slots.get_mut(k.idx as usize)?;
        if s.gen != k.gen || s.val.is_none() {
            return None;
        }
        self.free.push(k.idx);
        self.len -= 1;
        s.val.take()
    }
    pub fn get(&self, k: Key) -> Option<&T> {
        let s = self.slots.get(k.idx as usize)?;
        if s.gen == k.gen { s.val.as_ref() } else { None }
    }
    pub fn get_mut(&mut self, k: Key) -> Option<&mut T> {
        let s = self.slots.get_mut(k.idx as usize)?;
        if s.gen == k.gen { s.val.as_mut() } else { None }
    }
    pub fn contains(&self, k: Key) -> bool {
        self.get(k).is_some()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn keys(&self) -> Vec<Key> {
        self.iter().map(|(k, _)| k).collect()
    }
    pub fn iter(&self) -> impl Iterator<Item = (Key, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.val.as_ref().map(|v| (Key { idx: i as u32, gen: s.gen }, v)))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Key, &mut T)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(i, s)| {
                let g = s.gen;
                s.val.as_mut().map(|v| (Key { idx: i as u32, gen: g }, v))
            })
    }
}

impl<T> std::ops::Index<Key> for Slab<T> {
    type Output = T;
    fn index(&self, k: Key) -> &T {
        self.get(k).expect("stale slab key")
    }
}
impl<T> std::ops::IndexMut<Key> for Slab<T> {
    fn index_mut(&mut self, k: Key) -> &mut T {
        self.get_mut(k).expect("stale slab key")
    }
}
