pub(crate) struct AlignedBuffer<T> {
    ptr: *mut T,
    len: usize,
    layout: std::alloc::Layout,
}

impl<T> AlignedBuffer<T> {
    pub fn new(len: usize, align: usize) -> AlignedBuffer<T> {
        let layout =
            std::alloc::Layout::from_size_align(len * std::mem::size_of::<T>(), align).unwrap();
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) } as *mut T;

        AlignedBuffer { ptr, len, layout }
    }
}

impl<T> Drop for AlignedBuffer<T> {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.ptr as *mut u8, self.layout) }
    }
}

// Safety: AlignedBuffer owns a unique allocation. After construction, TiledHeightmap
// only reads from it — no concurrent mutation. Safe to share across threads.
unsafe impl<T: Send> Send for AlignedBuffer<T> {}
unsafe impl<T: Sync> Sync for AlignedBuffer<T> {}

impl<T> std::ops::Deref for AlignedBuffer<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl<T> std::ops::DerefMut for AlignedBuffer<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}
