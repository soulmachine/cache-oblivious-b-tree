use num_rational::{Ratio, Rational};
use std::cell::UnsafeCell;
use std::cmp::{Ord, Ordering};
use std::collections::VecDeque;
use std::convert::TryInto;
use std::fmt::{self, Debug};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ops::{Range, RangeInclusive};
use std::slice;
use std::sync::atomic::{AtomicPtr, AtomicU16, Ordering as AtomicOrdering};

#[derive(Debug, PartialEq, Eq, PartialOrd, Clone, Copy)]
enum Key<T: Ord> {
  Infimum,
  Value(T),
  Supremum,
}

impl<T: Ord> Ord for Key<T> {
  fn cmp(&self, other: &Self) -> Ordering {
    match (self, other) {
      (x, y) if x == y => Ordering::Equal,
      (Key::Infimum, _) | (_, Key::Supremum) => Ordering::Less,
      (Key::Supremum, _) | (_, Key::Infimum) => Ordering::Greater,
      (Key::Value(a), Key::Value(b)) => a.cmp(b),
    }
  }
}

#[derive(Debug)]
enum Marker<K, V> {
  Empty(u16),
  Move(u16, isize),
  InsertCell(u16, K, V),
  DeleteCell(u16, K),
}

impl<K, V> Marker<K, V> {
  fn version(&self) -> &u16 {
    match self {
      Marker::Empty(v)
      | Marker::Move(v, _)
      | Marker::InsertCell(v, _, _)
      | Marker::DeleteCell(v, _) => v,
    }
  }
}

struct Cell<'a, K: 'a, V: 'a> {
  version: AtomicU16,
  marker: Option<AtomicPtr<Marker<K, V>>>,
  key: UnsafeCell<Option<K>>,
  value: UnsafeCell<Option<V>>,
  _marker: PhantomData<&'a Marker<K, V>>,
}

impl<K, V> Drop for Cell<'_, K, V> {
  fn drop(&mut self) {
    let ptr = self.marker.take().unwrap();
    let marker = ptr.load(AtomicOrdering::Acquire);
    unsafe { Box::from_raw(marker) };
  }
}

impl<K: Debug, V: Debug> Debug for Cell<'_, K, V> {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    let version = self.version.load(AtomicOrdering::Acquire);
    let marker = unsafe { &*self.marker.as_ref().unwrap().load(AtomicOrdering::Acquire) };
    let key = unsafe { &*self.key.get() };
    let value = unsafe { &*self.value.get() };

    let mut dbg_struct = formatter.debug_struct("Cell");

    dbg_struct
      .field("version", &version)
      .field("marker", marker)
      .field("key", key)
      .field("value", value);

    dbg_struct.finish()
  }
}

struct Block<'a, K: Eq + Ord, V> {
  cell_slice_ptr: *const Cell<'a, K, V>,
  length: usize,
  _marker: PhantomData<&'a [Cell<'a, K, V>]>,
}

impl<K: Eq + Ord, V> Block<'_, K, V> {
  // pub fn get(&self, _key: K) -> (&AtomicU16, &Option<V>) {
  //   let cell = &self.cell_slice()[0];
  //   (&cell.version, unsafe { &*cell.value.get() })
  // }

  fn cell_slice(&self) -> &[Cell<K, V>] {
    unsafe { slice::from_raw_parts(self.cell_slice_ptr, self.length) }
  }
}

impl<K: Eq + Ord + Debug, V: Debug> Debug for Block<'_, K, V> {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter
      .debug_struct("Block")
      .field("cells", &format_args!("{:?}", self.cell_slice()))
      .finish()
  }
}

enum Node<'a, K: Eq + Ord, V: Copy> {
  Leaf(Key<K>, Block<'a, K, V>),
  Branch {
    min_rhs: Key<K>,
    left: MaybeUninit<*const Node<'a, K, V>>,
    right: MaybeUninit<*const Node<'a, K, V>>,
    _marker: PhantomData<&'a Node<'a, K, V>>,
  },
}

impl<K: Eq + Ord, V: Copy> Node<'_, K, V> {
  fn search_to_block(&self, key: Key<K>) -> &Block<'_, K, V> {
    match self {
      Node::Leaf(_, block) => block,
      Node::Branch {
        min_rhs,
        left,
        right,
        ..
      } => {
        let node = if &key < min_rhs {
          unsafe { &**left.assume_init_ref() }
        } else {
          unsafe { &**right.assume_init_ref() }
        };
        node.search_to_block(key)
      }
    }
  }
}

impl<K: Eq + Ord + Debug, V: Copy + Debug> Debug for Node<'_, K, V> {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    if formatter.alternate() {
      match self {
        Self::Leaf(_, block) => formatter.write_fmt(format_args!(
          "{}",
          format!("|- Leaf => {:?} @ {:p}", block, self)
        )),
        Self::Branch {
          min_rhs,
          left,
          right,
          ..
        } => formatter.write_fmt(format_args!(
          "|- Branch {{ min_rhs: {:?} }} @ {:p}\n{:ident$}{}\n{:ident$}{}",
          min_rhs,
          self,
          "|",
          format!(
            "{:#ident$?}",
            unsafe { &**left.assume_init_ref() },
            ident = formatter.width().unwrap_or(0) + 3
          ),
          "|",
          format!(
            "{:#ident$?}",
            unsafe { &**right.assume_init_ref() },
            ident = formatter.width().unwrap_or(0) + 3
          ),
          ident = formatter.width().unwrap_or(0) + 3,
        )),
      }
    } else {
      match self {
        Self::Leaf(_, block) => {
          formatter.write_fmt(format_args!("{}", format!("|- Leaf => {:?}", block)))
        }
        Self::Branch {
          min_rhs,
          left,
          right,
          ..
        } => formatter.write_fmt(format_args!(
          "|- Branch {{ min_rhs: {:?} }}\n{:ident$}{}\n{:ident$}{}",
          min_rhs,
          "|",
          format!(
            "{:ident$?}",
            unsafe { &**left.assume_init_ref() },
            ident = formatter.width().unwrap_or(0) + 3
          ),
          "|",
          format!(
            "{:ident$?}",
            unsafe { &**right.assume_init_ref() },
            ident = formatter.width().unwrap_or(0) + 3
          ),
          ident = formatter.width().unwrap_or(0) + 3,
        )),
      }
    }
  }
}

#[derive(Debug)]
struct Density {
  max_item_count: usize,
  range: RangeInclusive<Rational>,
}

#[derive(Debug)]
struct Config {
  density_scale: Vec<Density>,
}

pub struct StaticSearchTree<'a, K: Eq + Ord + 'a, V: Copy + 'a> {
  nodes: Box<[Node<'a, K, V>]>,
  cells: Box<[MaybeUninit<Cell<'a, K, V>>]>,
  active_cells_ptr_range: Range<*const Cell<'a, K, V>>,
  config: Config,
}

impl<'a, K: Eq + Ord, V: Copy> StaticSearchTree<'a, K, V> {
  fn root(&'a self) -> &Node<'a, K, V> {
    &self.nodes[0]
  }
}

impl<K: Eq + Ord + Debug, V: Copy + Debug> Debug for StaticSearchTree<'_, K, V> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match f.alternate() {
      true => f.write_fmt(format_args!("{:#?}", self.root())),
      false => f.write_fmt(format_args!("{:?}", self.root())),
    }
  }
}

impl<'a, K: 'a, V: 'a> StaticSearchTree<'a, K, V>
where
  K: Eq + Copy + Ord + std::fmt::Debug + Copy,
  V: std::fmt::Debug + Copy,
{
  pub fn new(num_keys: u32) -> Self {
    let mut cells = Self::allocate_leaf_cells(num_keys);
    let mut nodes = Self::allocate_nodes(cells.len());

    let mut leaves = Self::initialize_nodes(&mut *nodes, None);

    let size = num_keys;
    let slot_size = f32::log2(size as f32) as usize; // https://github.com/rust-lang/rust/issues/70887
    let left_buffer_space = cells.len() >> 2;
    let left_buffer_slots = left_buffer_space / slot_size;
    let mut slots = cells.chunks_exact_mut(slot_size).skip(left_buffer_slots);

    for leaf in leaves.iter_mut() {
      Self::finalize_leaf_node(leaf, slots.next().unwrap());
    }

    let active_cells_ptr_range = std::ops::Range {
      start: unsafe { cells[left_buffer_space].assume_init_ref() } as *const _,
      end: unsafe { cells[cells.len() - left_buffer_space].assume_init_ref() } as *const _,
    };

    let initialized_nodes = unsafe { nodes.assume_init() };

    let num_densities = f32::log2(cells.len() as f32) as isize;

    // max density for 2^num_densities cells: 1/2
    let t_min = Rational::new(1, 2);
    // max density for 2^1 cells: 1
    let t_max = Rational::from_integer(1);
    // min density for 2^num_densities cells: 1/4
    let p_max = Rational::new(1, 4);
    // min density for 2^1 cells: 1/8
    let p_min = Rational::new(1, 8);

    let t_delta = t_max - t_min;
    let p_delta = p_max - p_min;

    let density_scale = (1..=num_densities)
      .map(|i| Density {
        max_item_count: 1 << i,
        range: RangeInclusive::new(
          p_min + (Rational::new(i - 1, num_densities - 1)) * p_delta,
          t_max - (Rational::new(i - 1, num_densities - 1)) * t_delta,
        ),
      })
      .collect::<Vec<_>>();

    StaticSearchTree {
      cells,
      nodes: initialized_nodes,
      active_cells_ptr_range,
      config: Config { density_scale },
    }
  }

  pub fn find(&self, search_key: K) -> Option<V> {
    let block = self.root().search_to_block(Key::Value(search_key));
    let mut current_cell_ptr = block.cell_slice_ptr;

    loop {
      let cell = unsafe { &*current_cell_ptr };
      let version = cell.version.load(AtomicOrdering::SeqCst);
      let current_marker_raw = cell.marker.as_ref().unwrap().load(AtomicOrdering::SeqCst);
      let marker = unsafe { &*current_marker_raw };

      if version != *marker.version() {
        todo!("Read marker, perform action there");
      }

      let key: &Option<K> = unsafe { &*cell.key.get() };
      let is_filled_cell = key.is_some();

      if is_filled_cell {
        if key.unwrap() == search_key {
          return unsafe { *cell.value.get() };
        } else if key.unwrap() > search_key {
          return None;
        }
      }

      current_cell_ptr = unsafe { current_cell_ptr.add(1) };
      if self.active_cells_ptr_range.contains(&current_cell_ptr) {
        continue;
      } else {
        break;
      }

      break;
    }

    None
  }

  fn rebalance(&self, cell_ptr_start: *const Cell<K, V>, for_insertion: bool) {
    println!("Rebalancing...");
    let mut count = 1;
    let mut current_cell_ptr = cell_ptr_start;
    let mut cells_to_move: VecDeque<*const Cell<K, V>> = VecDeque::new();

    loop {
      let cell_idx =
        unsafe { current_cell_ptr.offset_from(self.cells[0].assume_init_ref() as *const Cell<K, V>) };
      println!("Current global cell index {:?}", cell_idx);

      let cell = unsafe { &*current_cell_ptr };
      let cell_key = unsafe { *cell.key.get() };
      if cell_key.is_some() {
        cells_to_move.push_front(cell);
        println!("Adding to queue: {:?}", cell);
      } else {
        println!("Empty cell...");
      }

      let numer = if for_insertion {
        cells_to_move.len() + 1
      } else {
        cells_to_move.len()
      };

      let current_density = Rational::new(numer.try_into().unwrap(), count.try_into().unwrap());

      if self.within_density_threshold(count, current_density) {
        println!("FOUND WINDOW [_; {}] TO FIT DENSITY THRESHOLD", count);
        break;
      }

      current_cell_ptr = unsafe { current_cell_ptr.add(1) };
      if self.active_cells_ptr_range.contains(&current_cell_ptr) {
        count += 1;
        continue;
      } else {
        todo!("We've reached the end of the initialized cell buffer, resize tree");
      }
    }

    // There are different strategies available for rebalancing
    // depending on the inserts expected in the system.
    //
    // To support many inserts in the same area, we want to shift
    // elements as far right as possible, while still maintaining
    // our density thresholds. Since we know we needed exactly
    // this # of cells to meet our density, one less than this
    // should let each region maximize their respective thresholds.

    // Starting at current_marker_raw, move the cells rightward
    // until their max density is reached

    // TODO: Can we use #compare_and_swap for neighbouring cells
    // to make sure we don't leave any cells unallocated?

    for cell_ptr in cells_to_move.iter() {
      let cell = unsafe { &*current_cell_ptr };
      let cell_key = unsafe { *cell.key.get() };
      if cell_key.is_some() {
        // TODO: I think we can overwrite these records since their contents have been moved...
      }

      let cell_to_move = unsafe { &**cell_ptr };
      let version = cell_to_move.version.load(AtomicOrdering::SeqCst);
      let current_marker_raw = cell_to_move
        .marker
        .as_ref()
        .unwrap()
        .load(AtomicOrdering::SeqCst);
      let marker = unsafe { &*current_marker_raw };
      let marker_version = *marker.version();

      if version != marker_version {
        todo!("Restart rebalance!");
      }

      let dest_index =
        unsafe { current_cell_ptr.offset_from(self.cells[0].assume_init_ref() as *const Cell<K, V>) };
      let new_marker = Box::new(Marker::Move(marker_version, dest_index));

      println!("Writing marker {:?}", new_marker);

      let new_marker_raw = Box::into_raw(new_marker);
      let prev_marker = cell_to_move.marker.as_ref().unwrap().compare_exchange(
        current_marker_raw,
        new_marker_raw,
        AtomicOrdering::SeqCst,
        AtomicOrdering::SeqCst,
      );

      if prev_marker.is_err() {
        // Marker has been updated by another process.
        // Deallocate memory, start loop over.
        unsafe { Box::from_raw(new_marker_raw) };
        todo!("Restart rebalance!");
      }

      unsafe {
        current_cell_ptr.as_ref().unwrap().key.get().write(*cell_to_move.key.get());
        current_cell_ptr.as_ref().unwrap().value.get().write(*cell_to_move.value.get());
        cell_to_move.key.get().write(None);
        cell_to_move.value.get().write(None);
        let new_version = version + 1;
        // reuse prev_marker box
        prev_marker.unwrap().write(Marker::Empty(new_version));
        // use compare_exchange_weak because we can safely fail here
        let _ = cell_to_move.marker.as_ref().unwrap().compare_exchange_weak(
          new_marker_raw,
          prev_marker.unwrap(),
          AtomicOrdering::SeqCst,
          AtomicOrdering::SeqCst,
        );
        let _ = cell_to_move.version.compare_exchange_weak(
          marker_version,
          new_version,
          AtomicOrdering::SeqCst,
          AtomicOrdering::SeqCst
        );
        // TODO: increment version, clear marker
      };


      current_cell_ptr = unsafe { current_cell_ptr.sub(1) };
      if self.active_cells_ptr_range.contains(&current_cell_ptr) {
        continue;
      } else {
        unreachable!("We've reached the end of the initialized cell buffer!");
      }
    }

    println!("REALLOCATED ALL CELLS");
  }

  fn within_density_threshold(&self, num_items: usize, current_density: Ratio<isize>) -> bool {
    let density = self
      .config
      .density_scale
      .iter()
      .find(|d| d.max_item_count >= num_items)
      .unwrap();

    density.range.contains(&current_density)
  }

  pub fn add(&mut self, key: K, value: V) -> bool {
    let block = self.root().search_to_block(Key::Value(key));

    let mut current_cell_ptr = block.cell_slice_ptr;
    let mut selected_cell: Option<&Cell<K, V>> = None;

    loop {
      let cell = unsafe { &*current_cell_ptr };
      let version = cell.version.load(AtomicOrdering::SeqCst);
      let current_marker_raw = cell.marker.as_ref().unwrap().load(AtomicOrdering::SeqCst);
      let marker = unsafe { &*current_marker_raw };

      if version != *marker.version() {
        todo!("Read marker, perform action there");
      }

      let cell_key = unsafe { *cell.key.get() };

      // cell is not empty
      if let Some(k) = cell_key {
        println!("Attempting to find slot for key {:?}", key);
        println!("Filled cell has key {:?}", k);
        if Key::Value(k) <= Key::Value(key) {
          selected_cell = Some(cell);

          current_cell_ptr = unsafe { current_cell_ptr.add(1) };
          if self.active_cells_ptr_range.contains(&current_cell_ptr) {
            continue;
          } else {
            todo!("We've reached the end of the initialized cell buffer, resize tree");
          }
        } else if selected_cell.is_none() {
          // we didn't find any cells that were <= our key, rebalance to make room
          self.rebalance(cell, true);
        }
      } else {
        selected_cell = None;
      }

      if let Some(cell_to_move) = selected_cell {
        // move cell to make room for insert
        self.rebalance(cell_to_move, true);
      }

      let marker_version = version + 1;

      let cell = selected_cell.unwrap_or(cell);

      let new_marker = Box::new(Marker::InsertCell(
        marker_version,
        key,
        value,
      ));
      let new_marker_raw = Box::into_raw(new_marker);
      let prev_marker = cell.marker.as_ref().unwrap().compare_exchange(
        current_marker_raw,
        new_marker_raw,
        AtomicOrdering::SeqCst,
        AtomicOrdering::SeqCst,
      );

      if prev_marker.is_err() {
        // Deallocate memory, try again next time
        unsafe { Box::from_raw(new_marker_raw) };
        // Marker has been updated by another process, start loop over
        continue;
      }

      // We now have exclusive access to the cell until we update `version`.
      // This works well for mutating through UnsafeCell<T>, but isn't really
      // "lock-free"...
      unsafe {
        cell.key.get().write(Some(key));
        cell.value.get().write(Some(value));
      };

      let next_version = marker_version + 1;

      // Reuse previous marker allocation
      unsafe { prev_marker.unwrap().write(Marker::Empty(next_version)) };
      cell
        .marker
        .as_ref()
        .unwrap()
        .swap(prev_marker.unwrap(), AtomicOrdering::SeqCst);
      cell.version.swap(next_version, AtomicOrdering::SeqCst);

      println!("INSERT SUCCESS: {:?}", cell);

      break;
    }

    true
  }

  pub fn print_cells(&self) -> () {
    let mut cell_ptr = unsafe { self.cells[0].assume_init_ref() } as *const Cell<K, V>;
    for i in 0..self.cells.len() {
      if self.active_cells_ptr_range.contains(&cell_ptr) {
        println!("[{}] {:?}", i, unsafe { &*cell_ptr });
      } else {
        println!("[{}] <(uninit)>", i);
      }
      unsafe { cell_ptr = cell_ptr.add(1) };
    }
  }

  fn allocate_leaf_cells(num_keys: u32) -> Box<[MaybeUninit<Cell<'a, K, V>>]> {
    let size = Self::values_mem_size(num_keys);
    println!("packed memory array [V; {:?}]", size);
    Box::<[Cell<K, V>]>::new_uninit_slice(size as usize)
  }

  fn allocate_nodes(leaf_count: usize) -> Box<[MaybeUninit<Node<'a, K, V>>]> {
    let size = leaf_count;
    // https://github.com/rust-lang/rust/issues/70887
    let slot_size = f32::log2(size as f32) as usize;
    let leaf_count = size / slot_size;
    let node_count = 2 * leaf_count - 1;
    println!("tree has {:?} leaves, {:?} nodes", leaf_count, node_count);
    Box::<[Node<K, V>]>::new_uninit_slice(node_count as usize)
  }

  fn assign_node_values<'b>(
    nodes: &'b mut [MaybeUninit<Node<'a, K, V>>],
    parent_node: Option<*mut *const Node<'a, K, V>>,
  ) -> Vec<&'b mut Node<'a, K, V>> {
    let num_nodes = nodes.len();
    assert!(num_nodes <= 3);

    for i in 0..num_nodes as usize {
      nodes[i].write(Node::Branch {
        min_rhs: Key::Supremum,
        left: MaybeUninit::uninit(),
        right: MaybeUninit::uninit(),
        _marker: PhantomData,
      });
    }

    if num_nodes == 1 {
      return vec![];
    }

    let left_node = unsafe { nodes[1].assume_init_ref() } as *const _;
    let right_node = unsafe { nodes[2].assume_init_ref() } as *const _;

    nodes[0].write(Node::Branch {
      min_rhs: Key::Supremum,
      left: MaybeUninit::new(left_node),
      right: MaybeUninit::new(right_node),
      _marker: PhantomData,
    });

    let (lhs, rhs) = nodes.split_at_mut(2);

    if let Some(node) = parent_node {
      unsafe { node.write(lhs[0].assume_init_ref() as *const _) };
    }

    let left_branch = unsafe { lhs[1].assume_init_mut() };
    let right_branch = unsafe { rhs[0].assume_init_mut() };

    return vec![left_branch, right_branch];
  }

  fn initialize_nodes<'b>(
    nodes: &'b mut [MaybeUninit<Node<'a, K, V>>],
    parent_node: Option<*mut *const Node<'a, K, V>>,
  ) -> Vec<&'b mut Node<'a, K, V>> {
    if nodes.len() <= 3 {
      return Self::assign_node_values(nodes, parent_node);
    }

    let (upper_mem, lower_mem) = Self::split_tree_memory(nodes);
    let num_lower_branches = upper_mem.len() + 1;

    let leaves_of_upper = Self::initialize_nodes(upper_mem, parent_node);

    let nodes_per_branch = lower_mem.len() / num_lower_branches;
    let mut branches = lower_mem.chunks_exact_mut(nodes_per_branch);

    leaves_of_upper
      .into_iter()
      .flat_map(|subtree_leaf| match subtree_leaf {
        Node::Leaf(_, _) => unreachable!(),
        Node::Branch { left, right, .. } => {
          let lhs_mem = branches.next().unwrap();
          let rhs_mem = branches.next().unwrap();
          let lhs = Self::initialize_nodes(lhs_mem, Some(left.as_mut_ptr()));
          let rhs = Self::initialize_nodes(rhs_mem, Some(right.as_mut_ptr()));
          lhs.into_iter().chain(rhs.into_iter())
        }
      })
      .collect::<Vec<_>>()
  }

  fn split_tree_memory<'b>(
    nodes: &'b mut [MaybeUninit<Node<'a, K, V>>],
  ) -> (
    &'b mut [MaybeUninit<Node<'a, K, V>>],
    &'b mut [MaybeUninit<Node<'a, K, V>>],
  ) {
    let height = f32::log2(nodes.len() as f32 + 1f32);
    let lower_height = ((height / 2f32).ceil() as u32).next_power_of_two();
    let upper_height = height as u32 - lower_height;

    let upper_subtree_length = 2 << (upper_height - 1);
    nodes.split_at_mut(upper_subtree_length - 1)
  }

  fn finalize_leaf_node<'b>(
    leaf: &'b mut Node<'a, K, V>,
    leaf_mem: &'b mut [MaybeUninit<Cell<'a, K, V>>],
  ) -> () {
    match leaf {
      Node::Branch { min_rhs, .. } => {
        let length = leaf_mem.len();
        let initialized_mem = Self::init_cell_block(leaf_mem);
        let ptr = initialized_mem as *const [Cell<K, V>] as *const Cell<K, V>;
        let block = Block {
          cell_slice_ptr: ptr,
          length,
          _marker: PhantomData,
        };
        *leaf = Node::Leaf(*min_rhs, block);
      }
      Node::Leaf(_, _) => (),
    };
  }

  fn init_cell_block<'b>(
    cell_memory: &'b mut [MaybeUninit<Cell<'a, K, V>>],
  ) -> &'b mut [Cell<'a, K, V>] {
    for cell in cell_memory.iter_mut() {
      let marker = Box::new(Marker::<K, V>::Empty(1));
      let ptr = Box::into_raw(marker);
      cell.write(Cell {
        version: AtomicU16::new(1),
        marker: Some(AtomicPtr::new(ptr)),
        key: UnsafeCell::new(None),
        value: UnsafeCell::new(None),
        _marker: PhantomData,
      });
    }
    unsafe { MaybeUninit::slice_assume_init_mut(cell_memory) }
  }

  fn values_mem_size(num_keys: u32) -> u32 {
    let t_min = 0.5;
    let p_max = 0.25;
    let ideal_density = (t_min - p_max) / 2f32;

    let length = num_keys as f32 / ideal_density;
    // To get a balanced tree, we need to find the
    // closest double-exponential number (x = 2^2^i)
    let clean_length = 2 << ((f32::log2(length).ceil() as u32).next_power_of_two() - 1);
    clean_length
  }
}
