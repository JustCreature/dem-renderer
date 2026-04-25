pub(crate) struct AlignedBuffer {
    ptr: *mut i16,
    len: usize,
    layout: std::alloc::Layout,
}

impl AlignedBuffer {
    pub fn new(len: usize, align: usize) -> AlignedBuffer {
        let layout =
            std::alloc::Layout::from_size_align(len * std::mem::size_of::<i16>(), align).unwrap();
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) } as *mut i16;

        AlignedBuffer { ptr, len, layout }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.ptr as *mut u8, self.layout) }
    }
}

// Safety: AlignedBuffer owns a unique allocation. After construction, TiledHeightmap
// only reads from it — no concurrent mutation. Safe to share across threads.
unsafe impl Send for AlignedBuffer {}
unsafe impl Sync for AlignedBuffer {}

impl std::ops::Deref for AlignedBuffer {
    type Target = [i16];

    fn deref(&self) -> &[i16] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl std::ops::DerefMut for AlignedBuffer {
    fn deref_mut(&mut self) -> &mut [i16] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}
