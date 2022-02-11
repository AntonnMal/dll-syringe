use std::{cell::RefCell, marker::PhantomData, mem, ptr::NonNull, rc::Rc};

use get_last_error::Win32Error;

use crate::{
    process_memory::{Allocation, DynamicMultiBufferAllocator, ProcessMemorySlice, RawAllocator},
    ProcessRef,
};

#[derive(Debug, Clone)]
pub struct RemoteBoxAllocator<'a>(Rc<RefCell<DynamicMultiBufferAllocator<'a>>>);

impl<'a> RemoteBoxAllocator<'a> {
    pub fn new(process: ProcessRef<'a>) -> Self {
        Self(Rc::new(RefCell::new(DynamicMultiBufferAllocator::new(
            process,
        ))))
    }

    pub fn process(&self) -> ProcessRef<'a> {
        self.0.borrow().process()
    }

    pub unsafe fn alloc_raw<T: ?Sized>(
        &mut self,
        size: usize,
    ) -> Result<RemoteBox<'a, T>, Win32Error> {
        let allocation = self.0.borrow_mut().alloc(size)?;
        Ok(RemoteBox::new(self.0.clone(), allocation))
    }
    pub fn alloc_uninit<T: Sized>(&mut self) -> Result<RemoteBox<'a, T>, Win32Error> {
        unsafe { self.alloc_raw(mem::size_of::<T>()) }
    }
    pub fn alloc_uninit_for<T: ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<RemoteBox<'a, T>, Win32Error> {
        unsafe { self.alloc_raw(mem::size_of_val(value)) }
    }
    pub fn alloc_and_copy<T: ?Sized>(&mut self, value: &T) -> Result<RemoteBox<'a, T>, Win32Error> {
        let b = self.alloc_uninit_for(value)?;
        b.write(value)?;
        Ok(b)
    }

    #[allow(dead_code)]
    fn free(&mut self, allocation: &Allocation) {
        self.0.borrow_mut().free(allocation);
    }
}

#[derive(Debug)]
pub struct RemoteBox<'a, T: ?Sized> {
    allocation: Allocation,
    allocator: Rc<RefCell<DynamicMultiBufferAllocator<'a>>>,
    phantom: PhantomData<T>,
}

impl<'a, T: ?Sized> RemoteBox<'a, T> {
    fn new(
        allocator: Rc<RefCell<DynamicMultiBufferAllocator<'a>>>,
        allocation: Allocation,
    ) -> Self {
        Self {
            allocation,
            allocator,
            phantom: PhantomData,
        }
    }

    pub fn process(&self) -> ProcessRef<'a> {
        self.allocator.borrow().process()
    }

    pub fn memory(&self) -> ProcessMemorySlice<'a> {
        unsafe {
            ProcessMemorySlice::from_raw_parts(
                self.allocation.as_raw_ptr(),
                self.allocation.len,
                self.process(),
            )
        }
    }

    pub fn write(&self, value: &T) -> Result<(), Win32Error> {
        self.memory().write_struct(0, value)
    }

    pub fn as_raw_ptr(&mut self) -> *mut u8 {
        self.allocation.as_raw_ptr()
    }
}

impl<'a, T: Sized> RemoteBox<'a, T> {
    pub fn read(&self) -> Result<T, Win32Error> {
        unsafe { self.memory().read_struct::<T>(0) }
    }

    pub fn as_ptr(&mut self) -> NonNull<T> {
        self.allocation.as_ptr().cast()
    }
}

impl<T: ?Sized> Drop for RemoteBox<'_, T> {
    fn drop(&mut self) {
        self.allocator.borrow_mut().free(&self.allocation);
    }
}
