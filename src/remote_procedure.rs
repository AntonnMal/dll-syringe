use iced_x86::{code_asm::*, IcedError};

use std::{
    ffi::{c_void, CString},
    lazy::OnceCell,
    marker::PhantomData,
    mem::{self},
    ptr::{self, NonNull},
};

use winapi::shared::minwindef::FARPROC;

use crate::{
    error::SyringeError,
    process_memory::{RemoteBox, RemoteBoxAllocator},
    ProcessModule, ProcessRef, Syringe,
};

type RemoteProcedurePtr = NonNull<c_void>;

#[cfg_attr(feature = "doc_cfg", doc(cfg(feature = "remote_procedure")))]
impl<'a> Syringe<'a> {
    /// Loads an exported function from the given module from the target process.
    /// The function does not have to be from an injected module.
    pub fn get_procedure<T: ?Sized, R>(
        &mut self,
        module: ProcessModule<'_>,
        name: &str,
    ) -> Result<Option<RemoteProcedure<'a, T, R>>, SyringeError> {
        match self.get_procedure_address(module, name) {
            Ok(Some(procedure)) => Ok(Some(RemoteProcedure::new(
                procedure,
                self.remote_allocator.clone(),
            ))),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Load the address of the given function from the given module in the remote process.
    ///
    /// # Panics
    /// This method panics if the given module is not loaded in the target process.
    pub fn get_procedure_address(
        &mut self,
        module: ProcessModule<'_>,
        name: impl AsRef<str>,
    ) -> Result<Option<RemoteProcedurePtr>, SyringeError> {
        assert!(
            module.process() == self.process,
            "trying to load a procedure from a module of a different process"
        );

        self.build_get_proc_address_stub()?;
        let stub = self.get_proc_address_stub.get_mut().unwrap();

        let name = name.as_ref();
        let mut name = self
            .remote_allocator
            .alloc_and_copy(CString::new(name).unwrap().as_bytes_with_nul())?;
        stub.parameter.write(&GetProcAddressParams {
            module_handle: module.handle() as u64,
            name: name.as_raw_ptr() as u64,
        })?;

        // clear the result
        stub.result.write(&ptr::null_mut())?;

        let exit_code = self.process.run_remote_thread(
            unsafe { mem::transmute(stub.code.as_raw_ptr()) },
            stub.parameter.as_raw_ptr().cast(),
        )?;
        Syringe::remote_exit_code_to_exception(exit_code)?;

        Ok(RemoteProcedurePtr::new(stub.result.read()?.cast()))
    }

    fn build_get_proc_address_stub(&mut self) -> Result<(), SyringeError> {
        self.get_proc_address_stub
            .get_or_try_init::<_, SyringeError>(|| {
                let inject_data = self
                    .inject_help_data
                    .get_or_try_init(|| Self::load_inject_help_data_for_process(self.process))?;

                let remote_get_proc_address = inject_data.get_proc_address_fn_ptr();

                let parameter = self
                    .remote_allocator
                    .alloc_uninit::<GetProcAddressParams>()?;
                let mut result = self.remote_allocator.alloc_uninit::<FARPROC>()?;

                // Allocate memory in remote process and build a method stub.
                let code = if self.process.is_x86()? {
                    Syringe::build_get_proc_address_x86(
                        remote_get_proc_address as *const _,
                        result.as_raw_ptr().cast(),
                    )
                    .unwrap()
                } else {
                    Syringe::build_get_proc_address_x64(
                        remote_get_proc_address as *const _,
                        result.as_raw_ptr().cast(),
                    )
                    .unwrap()
                };
                let function_stub = self.remote_allocator.alloc_and_copy(code.as_slice())?;
                function_stub.memory().flush_instruction_cache()?;

                Ok(RemoteProcedureStub {
                    code: function_stub,
                    parameter,
                    result,
                })
            })?;

        Ok(())
    }

    fn build_call_procedure_x86(
        procedure: *const c_void,
        return_buffer: *mut c_void,
    ) -> Result<Vec<u8>, IcedError> {
        assert!(!procedure.is_null());
        assert!(!return_buffer.is_null());
        assert_eq!(procedure as u32 as usize, procedure as usize);
        assert_eq!(return_buffer as u32 as usize, return_buffer as usize);

        let mut asm = CodeAssembler::new(32)?;

        asm.mov(eax, esp + 4)?; // load arg ptr (lpParameter) from stack
        asm.push(return_buffer as u32)?; // push result ptr onto stack
        asm.push(eax)?; // push arg ptr onto stack
        asm.mov(eax, procedure as u32)?; // load address of target function
        asm.call(eax)?; // call real_address
        asm.mov(eax, 0)?; // return 0
        asm.ret_1(4)?; // Restore stack ptr. (Callee cleanup)

        let code = asm.assemble(0x1234_5678)?;
        debug_assert_eq!(
            code,
            asm.assemble(0x1111_2222)?,
            "CallProcedure x86 stub is not location independent"
        );

        Ok(code)
    }

    fn build_call_procedure_x64(
        procedure: *const c_void,
        return_buffer: *mut c_void,
    ) -> Result<Vec<u8>, IcedError> {
        assert!(!procedure.is_null());
        assert!(!return_buffer.is_null());

        let mut asm = CodeAssembler::new(64)?;

        asm.sub(rsp, 40)?; // Re-align stack to 16 byte boundary +32 shadow space
        asm.mov(rdx, return_buffer as u64)?; // result ptr
        asm.mov(rcx, rcx)?; // arg ptr
        asm.mov(rax, procedure as u64)?;
        asm.call(rax)?;
        asm.mov(rax, 0u64)?; // return 0
        asm.add(rsp, 40)?; // Re-align stack to 16 byte boundary + shadow space.
        asm.mov(rax, 0u64)?; // return 0
        asm.ret()?; // Restore stack ptr. (Callee cleanup)

        let code = asm.assemble(0x1234_5678)?;
        debug_assert_eq!(
            code,
            asm.assemble(0x1111_2222)?,
            "CallProcedure x64 stub is not location independent"
        );

        Ok(code)
    }

    fn build_get_proc_address_x86(
        get_proc_address: *const c_void,
        return_buffer: *mut c_void,
    ) -> Result<Vec<u8>, IcedError> {
        assert!(!get_proc_address.is_null());
        assert!(!return_buffer.is_null());
        assert_eq!(get_proc_address as u32 as usize, get_proc_address as usize);
        assert_eq!(return_buffer as u32 as usize, return_buffer as usize);

        // assembly code from https://github.com/Reloaded-Project/Reloaded.Injector/blob/77a9a87392cc75fa087d7004e8cdef054e880428/Source/Reloaded.Injector/Shellcode.cs#L159
        // mov eax, dword [esp + 4]         // CreateRemoteThread lpParameter
        // push dword [eax + 8]             // lpProcName
        // push dword [eax + 0]             // hModule
        // call dword [dword GetProcAddress]
        // mov dword [dword ReturnAddress], eax
        // ret 4                           // Restore stack ptr. (Callee cleanup)
        let mut asm = CodeAssembler::new(32)?;

        asm.mov(eax, esp + 4)?; // CreateRemoteThread lpParameter
        asm.push(dword_ptr(eax + 8))?; // lpProcName
        asm.push(dword_ptr(eax + 0))?; // hModule
        asm.mov(eax, get_proc_address as u32)?;
        asm.call(eax)?;
        asm.mov(dword_ptr(return_buffer as u32), eax)?;
        asm.mov(eax, 0)?; // return 0
        asm.ret_1(4)?; // Restore stack ptr. (Callee cleanup)

        let code = asm.assemble(0x1234_5678)?;
        debug_assert_eq!(
            code,
            asm.assemble(0x1111_2222)?,
            "GetProcAddress x86 stub is not location independent"
        );

        Ok(code)
    }

    fn build_get_proc_address_x64(
        get_proc_address: *const c_void,
        return_buffer: *mut c_void,
    ) -> Result<Vec<u8>, IcedError> {
        assert!(!get_proc_address.is_null());
        assert!(!return_buffer.is_null());

        // assembly code from https://github.com/Reloaded-Project/Reloaded.Injector/blob/77a9a87392cc75fa087d7004e8cdef054e880428/Source/Reloaded.Injector/Shellcode.cs#L188
        //                                      // CreateRemoteThread lpParameter @ ECX
        // sub rsp, 40                          // Re-align stack to 16 byte boundary +32 shadow space
        // mov rdx, qword [qword rcx + 8]       // lpProcName
        // mov rcx, qword [qword rcx + 0]       // hModule
        // call qword [qword GetProcAddress]    // [replaced with indirect call]
        // mov qword [qword ReturnAddress], rax
        // add rsp, 40                          // Re-align stack to 16 byte boundary + shadow space.
        // ret
        let mut asm = CodeAssembler::new(64).unwrap();

        asm.sub(rsp, 40)?; // Re-align stack to 16 byte boundary +32 shadow space
        asm.mov(rdx, qword_ptr(rcx + 8))?; // lpProcName
        asm.mov(rcx, qword_ptr(rcx + 0))?; // hModule
        asm.mov(rax, get_proc_address as u64)?;
        asm.call(rax)?;
        asm.mov(qword_ptr(return_buffer as u64), rax)?;
        asm.mov(rax, 0u64)?; // return 0
        asm.add(rsp, 40)?; // Re-align stack to 16 byte boundary + shadow space.
        asm.ret()?; // Restore stack ptr. (Callee cleanup)

        let code = asm.assemble(0x1234_5678)?;
        debug_assert_eq!(
            code,
            asm.assemble(0x1111_2222)?,
            "GetProcAddress x64 stub is not location independent"
        );

        Ok(code)
    }
}

/// A remote procedure from a module of a foreign process.
/// The procedure abides by the `extern "system" fn(*const T, *mut R)` signature.
#[cfg_attr(feature = "doc_cfg", doc(cfg(feature = "remote_procedure")))]
#[derive(Debug)]
pub struct RemoteProcedure<'a, T: ?Sized, R> {
    ptr: RemoteProcedurePtr,
    stub: OnceCell<RemoteProcedureStub<'a, T, R>>,
    remote_allocator: RemoteBoxAllocator<'a>,
    phantom: PhantomData<fn(T) -> R>,
}

impl<'a, T: ?Sized, R> RemoteProcedure<'a, T, R> {
    fn new(ptr: RemoteProcedurePtr, remote_allocator: RemoteBoxAllocator<'a>) -> Self {
        Self {
            ptr,
            remote_allocator,
            stub: OnceCell::new(),
            phantom: PhantomData,
        }
    }

    /// Returns the process that this remote procedure is from.
    pub fn process(&self) -> ProcessRef<'a> {
        self.remote_allocator.process()
    }

    /// Returns the underlying pointer to the remote procedure.
    pub const fn as_ptr(&self) -> *const c_void {
        self.ptr.as_ptr()
    }
}

impl<'a, T, R> RemoteProcedure<'a, T, R> {
    /// Calls the remote procedure with the given argument.
    /// As the argument is copied to the memory of the remote process, changes made in the called function will not be reflected in the local copy.
    pub fn call(&mut self, arg: &T) -> Result<R, SyringeError> {
        let stub = if let Some(stub) = self.stub.get_mut() {
            stub
        } else {
            self.stub
                .set(Self::build_stub(
                    self.ptr.as_ptr(),
                    &mut self.remote_allocator
                )?)
                .unwrap_or_else(|_| unreachable!());
            self.stub.get_mut().unwrap()
        };

        stub.call(arg)
    }

    fn build_stub(
        procedure: *const c_void,
        remote_allocator: &mut RemoteBoxAllocator<'a>,
    ) -> Result<RemoteProcedureStub<'a, T, R>, SyringeError> {
        let parameter = remote_allocator.alloc_uninit::<T>()?;
        let mut result = remote_allocator.alloc_uninit::<R>()?;

        let code = if remote_allocator.process().is_x86()? {
            Syringe::build_call_procedure_x86(procedure, result.as_raw_ptr().cast()).unwrap()
        } else {
            Syringe::build_call_procedure_x64(procedure, result.as_raw_ptr().cast()).unwrap()
        };
        let code = remote_allocator.alloc_and_copy(code.as_slice())?;
        code.memory().flush_instruction_cache()?;

        Ok(RemoteProcedureStub {
            code,
            parameter,
            result,
        })
    }
}

/*
TODO: finish the payload part of this code.
impl<'a, T: ?Sized, R> RemoteProcedure<'a, T, R> {
    /// Calls the remote procedure with the given argument.
    /// As the argument is copied to the memory of the remote process, changes made in the called function will not be reflected in the local copy.
    pub fn call(&mut self, arg: &T) -> Result<R, SyringeError> {
        let stub = if let Some(stub) = self.stub.get_mut() {
            // Stub already built -> check if argument will fit in the reserved parameter memory and if not, rebuild the stub
            if mem::size_of_val(arg) <= stub.parameter.memory().len() {
                stub
            } else {
                self.stub
                    .set(Self::build_stub(
                        self.ptr.as_ptr(),
                        &mut self.remote_allocator,
                        arg,
                    )?)
                    .unwrap_or_else(|_| unreachable!());
                self.stub.get_mut().unwrap()
            }
        } else {
            // Stub not built -> build it
            self.stub
                .set(Self::build_stub(
                    self.ptr.as_ptr(),
                    &mut self.remote_allocator,
                    arg,
                )?)
                .unwrap_or_else(|_| unreachable!());
            self.stub.get_mut().unwrap()
        };

        stub.call(arg)
    }

    fn build_stub(
        procedure: *const c_void,
        remote_allocator: &mut RemoteBoxAllocator<'a>,
        arg: &T,
    ) -> Result<RemoteProcedureStub<'a, T, R>, SyringeError> {
        let parameter = remote_allocator.alloc_uninit_for(arg)?;
        let mut result = remote_allocator.alloc_uninit::<R>()?;

        let code = if remote_allocator.process().is_x86()? {
            Syringe::build_call_procedure_x86(procedure, result.as_raw_ptr().cast()).unwrap()
        } else {
            Syringe::build_call_procedure_x64(procedure, result.as_raw_ptr().cast()).unwrap()
        };
        let code = remote_allocator.alloc_and_copy(code.as_slice())?;
        code.memory().flush_instruction_cache()?;

        Ok(RemoteProcedureStub {
            code,
            parameter,
            result,
        })
    }
}
*/

#[derive(Debug)]
#[repr(C)]
pub(crate) struct GetProcAddressParams {
    module_handle: u64,
    name: u64,
}

#[derive(Debug)]
pub(crate) struct RemoteProcedureStub<'a, T: ?Sized, R> {
    pub code: RemoteBox<'a, [u8]>,
    pub parameter: RemoteBox<'a, T>,
    pub result: RemoteBox<'a, R>,
}

impl<'a, T: ?Sized, R> RemoteProcedureStub<'a, T, R> {
    pub fn call(&mut self, arg: &T) -> Result<R, SyringeError> {
        self.parameter.write(arg)?;
        let exit_code = self.code.memory().process().run_remote_thread(
            unsafe { mem::transmute(self.code.as_raw_ptr()) },
            self.parameter.as_raw_ptr().cast(),
        )?;
        Syringe::remote_exit_code_to_exception(exit_code)?;

        Ok(self.result.read()?)
    }
}
