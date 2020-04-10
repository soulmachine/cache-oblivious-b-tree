#![feature(new_uninit)]
#![feature(alloc_layout_extra)]

extern crate alloc;

use std::fmt::Debug;
use std::marker::Copy;
use std::cmp::Ord;
use std::rc::Rc;
use std::cell::RefCell;

mod packed_data;
use packed_data::{PackedData};

#[derive(Debug)]
enum BinaryTreeEntry<K> {
    Node(BinaryTree<K>),
    Leaf { key: K, index: usize },
}

#[derive(Debug)]
struct BinaryTree<K> {
    key: K,
    left: Rc<RefCell<Option<BinaryTreeEntry<K>>>>,
    right: Rc<RefCell<Option<BinaryTreeEntry<K>>>>
}

fn convert_leaf_to_node<K: Copy>(leaf: &BinaryTreeEntry<K>) -> BinaryTreeEntry<K> {
    match leaf {
        BinaryTreeEntry::Leaf { key, index } => {
            let new_cell = Rc::new(RefCell::new(None));
            BinaryTreeEntry::Node(
                BinaryTree {
                    key: *key,
                    left: Rc::clone(&new_cell),
                    right: Rc::new(RefCell::new(Some(BinaryTreeEntry::Leaf { key: *key, index: *index })))
                }
            )
        },
        BinaryTreeEntry::Node(_) => unreachable!(),
    }
}

impl <K: Copy + Ord + Debug> BinaryTree<K> {
    fn search(&self, search_key: K) -> Option<usize> {
        if search_key < self.key {
            match &*self.left.borrow() {
                Some(BinaryTreeEntry::Leaf { key, index }) => if *key == search_key { Some(*index) } else { None },
                Some(BinaryTreeEntry::Node(tree)) => tree.search(search_key),
                None => None
            }
        } else {
            match &*self.right.borrow() {
                Some(BinaryTreeEntry::Leaf { key, index }) => if *key == search_key { Some(*index) } else { None },
                Some(BinaryTreeEntry::Node(tree)) => tree.search(search_key),
                None => None
            }
        }
    }

    fn fetch_insertion_cell(&self, insertion_key: K) -> Rc<RefCell<Option<BinaryTreeEntry<K>>>> {
        if insertion_key < self.key {
            let mut left_entry = self.left.borrow_mut();

            match &*left_entry {
                None => Rc::clone(&self.left),
                Some(entry) => {
                    match entry {
                        BinaryTreeEntry::Leaf { key: _, index: _ } => {
                            let node = convert_leaf_to_node(entry);
                            let cell = match &node {
                                BinaryTreeEntry::Node(tree) => Rc::clone(&tree.left),
                                BinaryTreeEntry::Leaf { key: _, index: _ } => unreachable!()
                            };
                            left_entry.replace(node);
                            cell
                        },
                        BinaryTreeEntry::Node(tree) => tree.fetch_insertion_cell(insertion_key)
                    }
                }
            }
        } else {
            let mut right_entry = self.right.borrow_mut();
            match &*right_entry {
                None => Rc::clone(&self.right),
                Some(entry) => {
                    match entry {
                        BinaryTreeEntry::Leaf { key, index: _ } if *key == insertion_key => {
                            // TODO use index to mark for cleanup
                            Rc::clone(&self.right)
                        },
                        BinaryTreeEntry::Leaf { key: _, index: _ } => {
                            let node = convert_leaf_to_node(entry);
                            let cell = match &node {
                                BinaryTreeEntry::Node(tree) => Rc::clone(&tree.left),
                                BinaryTreeEntry::Leaf { key: _, index: _ } => unreachable!()
                            };
                            right_entry.replace(node);
                            cell
                        },
                        BinaryTreeEntry::Node(tree) => tree.fetch_insertion_cell(insertion_key)
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct CacheObliviousBTreeMap<K, V> {
    packed_dataset: PackedData<V>,
    tree: Option<BinaryTree<K>>,
    // temporary
    next_loc: usize,
}

impl <K, V> CacheObliviousBTreeMap<K, V> {
    pub fn new() -> Self {
        CacheObliviousBTreeMap {
            packed_dataset: PackedData::new(32),
            tree: None,
            next_loc: 0,
        }
    }
}

impl <K: Copy + Debug + Ord, V: Copy + Debug> CacheObliviousBTreeMap<K, V> {
    pub fn get(&self, key: K) -> Option<&V> {
        self.tree
            .as_ref()
            .and_then(|t| t.search(key))
            .map(|idx| self.packed_dataset.get(idx))
    }

    pub fn insert(&mut self, key: K, value: V) {
        let index = self.next_cache_location();
        self.packed_dataset.set(index, value);
        let new_leaf = BinaryTreeEntry::Leaf { key, index };

        match self.tree.as_ref() {
            None => {
                let node =  Rc::new(RefCell::new(Some(new_leaf)));
                self.tree = Some(BinaryTree { key, left: Rc::new(RefCell::new(None)), right: Rc::clone(&node) });
            },
            Some(tree) => {
                let cell = tree.fetch_insertion_cell(key);
                cell.replace(Some(new_leaf));
            }
        }
    }

    // TODO PackedData should manage this
    #[inline]
    fn next_cache_location(&mut self) -> usize {
        let loc = self.next_loc;
        self.next_loc = (loc + 1) % 32;
        loc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let mut map = CacheObliviousBTreeMap::new();
        map.insert(5, "Hello");
        map.insert(3, "World");
        map.insert(2, "!");

        assert_eq!(map.get(5), Some(&"Hello"));
        assert_eq!(map.get(4), None);
        assert_eq!(map.get(3), Some(&"World"));
        assert_eq!(map.get(2), Some(&"!"));
    }
}
