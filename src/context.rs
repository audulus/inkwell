//! A `Context` is an opaque owner and manager of core global data.

#[llvm_versions(7.0..=latest)]
use crate::InlineAsmDialect;
use libc::c_void;
#[llvm_versions(3.6..7.0)]
use llvm_sys::core::LLVMConstInlineAsm;
#[llvm_versions(12.0..=latest)]
use llvm_sys::core::LLVMCreateTypeAttribute;
#[llvm_versions(7.0..=latest)]
use llvm_sys::core::LLVMGetInlineAsm;
#[llvm_versions(6.0..=latest)]
use llvm_sys::core::LLVMMetadataTypeInContext;
use llvm_sys::core::{
    LLVMAppendBasicBlockInContext, LLVMConstStringInContext, LLVMConstStructInContext, LLVMContextCreate,
    LLVMContextDispose, LLVMContextSetDiagnosticHandler, LLVMCreateBuilderInContext, LLVMDoubleTypeInContext,
    LLVMFP128TypeInContext, LLVMFloatTypeInContext, LLVMGetGlobalContext, LLVMGetMDKindIDInContext,
    LLVMHalfTypeInContext, LLVMInsertBasicBlockInContext, LLVMInt16TypeInContext, LLVMInt1TypeInContext,
    LLVMInt32TypeInContext, LLVMInt64TypeInContext, LLVMInt8TypeInContext, LLVMIntTypeInContext, LLVMMDNodeInContext,
    LLVMMDStringInContext, LLVMModuleCreateWithNameInContext, LLVMPPCFP128TypeInContext, LLVMStructCreateNamed,
    LLVMStructTypeInContext, LLVMVoidTypeInContext, LLVMX86FP80TypeInContext,
};
#[llvm_versions(3.9..=latest)]
use llvm_sys::core::{LLVMCreateEnumAttribute, LLVMCreateStringAttribute};
use llvm_sys::ir_reader::LLVMParseIRInContext;
use llvm_sys::prelude::{LLVMContextRef, LLVMDiagnosticInfoRef, LLVMTypeRef, LLVMValueRef};
use llvm_sys::target::{LLVMIntPtrTypeForASInContext, LLVMIntPtrTypeInContext};
use once_cell::sync::Lazy;
use parking_lot::{Mutex, MutexGuard};

#[llvm_versions(3.9..=latest)]
use crate::attributes::Attribute;
use crate::basic_block::BasicBlock;
use crate::builder::Builder;
use crate::memory_buffer::MemoryBuffer;
use crate::module::Module;
use crate::support::{to_c_str, LLVMString};
use crate::targets::TargetData;
#[llvm_versions(6.0..=latest)]
use crate::types::MetadataType;
use crate::types::{AnyTypeEnum, AsTypeRef, BasicTypeEnum, FloatType, FunctionType, IntType, StructType, VoidType};
use crate::values::{
    AsValueRef, BasicMetadataValueEnum, BasicValueEnum, FunctionValue, MetadataValue, PointerValue, StructValue,
    VectorValue,
};
use crate::AddressSpace;
#[cfg(feature = "internal-getters")]
use crate::LLVMReference;

use std::marker::PhantomData;
use std::mem::{forget, ManuallyDrop};
use std::ops::Deref;
use std::ptr;
use std::thread_local;

// The idea of using a Mutex<Context> here and a thread local'd MutexGuard<Context> in
// GLOBAL_CTX_LOCK is to ensure two things:
// 1) Only one thread has access to the global context at a time.
// 2) The thread has shared access across different points in the thread.
// This is still technically unsafe because another program in the same process
// could also be accessing the global context via the C API. `get_global` has been
// marked unsafe for this reason. Iff this isn't the case then this should be fully safe.
static GLOBAL_CTX: Lazy<Mutex<Context>> = Lazy::new(|| unsafe { Mutex::new(Context::new(LLVMGetGlobalContext())) });

thread_local! {
    pub(crate) static GLOBAL_CTX_LOCK: Lazy<MutexGuard<'static, Context>> = Lazy::new(|| {
        GLOBAL_CTX.lock()
    });
}

/// A `Context` is a container for all LLVM entities including `Module`s.
///
/// A `Context` is not thread safe and cannot be shared across threads. Multiple `Context`s
/// can, however, execute on different threads simultaneously according to the LLVM docs.
#[derive(Debug, PartialEq, Eq)]
pub struct Context {
    pub(crate) context: LLVMContextRef,
}

unsafe impl Send for Context {}

impl Context {
    pub(crate) unsafe fn new(context: LLVMContextRef) -> Self {
        assert!(!context.is_null());

        Context { context }
    }

    /// Creates a new `Context`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// ```
    pub fn create() -> Self {
        unsafe { Context::new(LLVMContextCreate()) }
    }

    /// Gets a `Mutex<Context>` which points to the global context singleton.
    /// This function is marked unsafe because another program within the same
    /// process could easily gain access to the same LLVM context pointer and bypass
    /// our `Mutex`. Therefore, using `Context::create()` is the preferred context
    /// creation function when you do not specifically need the global context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = unsafe {
    ///     Context::get_global(|_global_context| {
    ///         // do stuff
    ///     })
    /// };
    /// ```
    pub unsafe fn get_global<F, R>(func: F) -> R
    where
        F: FnOnce(&Context) -> R,
    {
        GLOBAL_CTX_LOCK.with(|lazy| func(&*lazy))
    }

    /// Creates a new `Builder` for a `Context`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let builder = context.create_builder();
    /// ```
    pub fn create_builder(&self) -> Builder {
        unsafe { Builder::new(LLVMCreateBuilderInContext(self.context)) }
    }

    /// Creates a new `Module` for a `Context`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let module = context.create_module("my_module");
    /// ```
    pub fn create_module(&self, name: &str) -> Module {
        let c_string = to_c_str(name);

        unsafe { Module::new(LLVMModuleCreateWithNameInContext(c_string.as_ptr(), self.context)) }
    }

    /// Creates a new `Module` for the current `Context` from a `MemoryBuffer`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let module = context.create_module("my_module");
    /// let builder = context.create_builder();
    /// let void_type = context.void_type();
    /// let fn_type = void_type.fn_type(&[], false);
    /// let fn_val = module.add_function("my_fn", fn_type, None);
    /// let basic_block = context.append_basic_block(fn_val, "entry");
    ///
    /// builder.position_at_end(basic_block);
    /// builder.build_return(None);
    ///
    /// let memory_buffer = module.write_bitcode_to_memory();
    ///
    /// let module2 = context.create_module_from_ir(memory_buffer).unwrap();
    /// ```
    // REVIEW: I haven't yet been able to find docs or other wrappers that confirm, but my suspicion
    // is that the method needs to take ownership of the MemoryBuffer... otherwise I see what looks like
    // a double free in valgrind when the MemoryBuffer drops so we are `forget`ting MemoryBuffer here
    // for now until we can confirm this is the correct thing to do
    pub fn create_module_from_ir(&self, memory_buffer: MemoryBuffer) -> Result<Module, LLVMString> {
        let mut module = ptr::null_mut();
        let mut err_str = ptr::null_mut();

        let code =
            unsafe { LLVMParseIRInContext(self.context, memory_buffer.memory_buffer, &mut module, &mut err_str) };

        forget(memory_buffer);

        if code == 0 {
            unsafe {
                return Ok(Module::new(module));
            }
        }

        unsafe { Err(LLVMString::new(err_str)) }
    }

    /// Creates a inline asm function pointer.
    ///
    /// # Example
    /// ```no_run
    /// use std::convert::TryFrom;
    /// use inkwell::context::Context;
    /// use inkwell::values::CallableValue;
    ///
    /// let context = Context::create();
    /// let module = context.create_module("my_module");
    /// let builder = context.create_builder();
    /// let void_type = context.void_type();
    /// let fn_type = void_type.fn_type(&[], false);
    /// let fn_val = module.add_function("my_fn", fn_type, None);
    /// let basic_block = context.append_basic_block(fn_val, "entry");
    ///
    /// builder.position_at_end(basic_block);
    /// let asm_fn = context.i64_type().fn_type(&[context.i64_type().into(), context.i64_type().into()], false);
    /// let asm = context.create_inline_asm(asm_fn, "syscall".to_string(), "=r,{rax},{rdi}".to_string(), true, false, None, false);
    /// let params = &[context.i64_type().const_int(60, false).into(), context.i64_type().const_int(1, false).into()];
    /// let callable_value = CallableValue::try_from(asm).unwrap();
    /// builder.build_call(callable_value, params, "exit");
    /// builder.build_return(None);
    /// ```
    #[llvm_versions(13.0..=latest)]
    pub fn create_inline_asm(
        &self,
        ty: FunctionType,
        mut assembly: String,
        mut constraints: String,
        sideeffects: bool,
        alignstack: bool,
        dialect: Option<InlineAsmDialect>,
        can_throw: bool,
    ) -> PointerValue {
        let can_throw_llvmbool = can_throw as i32;

        let value = unsafe {
            LLVMGetInlineAsm(
                ty.as_type_ref(),
                assembly.as_mut_ptr() as *mut ::libc::c_char,
                assembly.len(),
                constraints.as_mut_ptr() as *mut ::libc::c_char,
                constraints.len(),
                sideeffects as i32,
                alignstack as i32,
                dialect.unwrap_or(InlineAsmDialect::ATT).into(),
                can_throw_llvmbool,
            )
        };

        unsafe { PointerValue::new(value) }
    }
    /// Creates a inline asm function pointer.
    ///
    /// # Example
    /// ```no_run
    /// use std::convert::TryFrom;
    /// use inkwell::context::Context;
    /// use inkwell::values::CallableValue;
    ///
    /// let context = Context::create();
    /// let module = context.create_module("my_module");
    /// let builder = context.create_builder();
    /// let void_type = context.void_type();
    /// let fn_type = void_type.fn_type(&[], false);
    /// let fn_val = module.add_function("my_fn", fn_type, None);
    /// let basic_block = context.append_basic_block(fn_val, "entry");
    ///
    /// builder.position_at_end(basic_block);
    /// let asm_fn = context.i64_type().fn_type(&[context.i64_type().into(), context.i64_type().into()], false);
    /// let asm = context.create_inline_asm(asm_fn, "syscall".to_string(), "=r,{rax},{rdi}".to_string(), true, false, None);
    /// let params = &[context.i64_type().const_int(60, false).into(), context.i64_type().const_int(1, false).into()];
    /// let callable_value = CallableValue::try_from(asm).unwrap();
    /// builder.build_call(callable_value, params, "exit");
    /// builder.build_return(None);
    /// ```
    #[llvm_versions(7.0..=12.0)]
    pub fn create_inline_asm(
        &self,
        ty: FunctionType,
        mut assembly: String,
        mut constraints: String,
        sideeffects: bool,
        alignstack: bool,
        dialect: Option<InlineAsmDialect>,
    ) -> PointerValue {
        let value = unsafe {
            LLVMGetInlineAsm(
                ty.as_type_ref(),
                assembly.as_mut_ptr() as *mut ::libc::c_char,
                assembly.len(),
                constraints.as_mut_ptr() as *mut ::libc::c_char,
                constraints.len(),
                sideeffects as i32,
                alignstack as i32,
                dialect.unwrap_or(InlineAsmDialect::ATT).into(),
            )
        };

        unsafe { PointerValue::new(value) }
    }
    /// Creates a inline asm function pointer.
    ///
    /// # Example
    /// ```no_run
    /// use std::convert::TryFrom;
    /// use inkwell::context::Context;
    /// use inkwell::values::CallableValue;
    ///
    /// let context = Context::create();
    /// let module = context.create_module("my_module");
    /// let builder = context.create_builder();
    /// let void_type = context.void_type();
    /// let fn_type = void_type.fn_type(&[], false);
    /// let fn_val = module.add_function("my_fn", fn_type, None);
    /// let basic_block = context.append_basic_block(fn_val, "entry");
    ///
    /// builder.position_at_end(basic_block);
    /// let asm_fn = context.i64_type().fn_type(&[context.i64_type().into(), context.i64_type().into()], false);
    /// let asm = context.create_inline_asm(asm_fn, "syscall".to_string(), "=r,{rax},{rdi}".to_string(), true, false);
    /// let params = &[context.i64_type().const_int(60, false).into(), context.i64_type().const_int(1, false).into()];
    /// let callable_value = CallableValue::try_from(asm).unwrap();
    /// builder.build_call(callable_value, params, "exit");
    /// builder.build_return(None);
    /// ```
    #[llvm_versions(3.6..7.0)]
    pub fn create_inline_asm(
        &self,
        ty: FunctionType,
        assembly: String,
        constraints: String,
        sideeffects: bool,
        alignstack: bool,
    ) -> PointerValue {
        let value = unsafe {
            LLVMConstInlineAsm(
                ty.as_type_ref(),
                assembly.as_ptr() as *const ::libc::c_char,
                constraints.as_ptr() as *const ::libc::c_char,
                sideeffects as i32,
                alignstack as i32,
            )
        };

        unsafe { PointerValue::new(value) }
    }

    /// Gets the `VoidType`. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let void_type = context.void_type();
    ///
    /// assert_eq!(*void_type.get_context(), context);
    /// ```
    pub fn void_type(&self) -> VoidType {
        unsafe { VoidType::new(LLVMVoidTypeInContext(self.context)) }
    }

    /// Gets the `IntType` representing 1 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let bool_type = context.bool_type();
    ///
    /// assert_eq!(bool_type.get_bit_width(), 1);
    /// assert_eq!(*bool_type.get_context(), context);
    /// ```
    pub fn bool_type(&self) -> IntType {
        unsafe { IntType::new(LLVMInt1TypeInContext(self.context)) }
    }

    /// Gets the `IntType` representing 8 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let i8_type = context.i8_type();
    ///
    /// assert_eq!(i8_type.get_bit_width(), 8);
    /// assert_eq!(*i8_type.get_context(), context);
    /// ```
    pub fn i8_type(&self) -> IntType {
        unsafe { IntType::new(LLVMInt8TypeInContext(self.context)) }
    }

    /// Gets the `IntType` representing 16 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let i16_type = context.i16_type();
    ///
    /// assert_eq!(i16_type.get_bit_width(), 16);
    /// assert_eq!(*i16_type.get_context(), context);
    /// ```
    pub fn i16_type(&self) -> IntType {
        unsafe { IntType::new(LLVMInt16TypeInContext(self.context)) }
    }

    /// Gets the `IntType` representing 32 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let i32_type = context.i32_type();
    ///
    /// assert_eq!(i32_type.get_bit_width(), 32);
    /// assert_eq!(*i32_type.get_context(), context);
    /// ```
    pub fn i32_type(&self) -> IntType {
        unsafe { IntType::new(LLVMInt32TypeInContext(self.context)) }
    }

    /// Gets the `IntType` representing 64 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let i64_type = context.i64_type();
    ///
    /// assert_eq!(i64_type.get_bit_width(), 64);
    /// assert_eq!(*i64_type.get_context(), context);
    /// ```
    pub fn i64_type(&self) -> IntType {
        unsafe { IntType::new(LLVMInt64TypeInContext(self.context)) }
    }

    /// Gets the `IntType` representing 128 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let i128_type = context.i128_type();
    ///
    /// assert_eq!(i128_type.get_bit_width(), 128);
    /// assert_eq!(*i128_type.get_context(), context);
    /// ```
    pub fn i128_type(&self) -> IntType {
        // REVIEW: The docs says there's a LLVMInt128TypeInContext, but
        // it might only be in a newer version

        self.custom_width_int_type(128)
    }

    /// Gets the `IntType` representing a custom bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let i42_type = context.custom_width_int_type(42);
    ///
    /// assert_eq!(i42_type.get_bit_width(), 42);
    /// assert_eq!(*i42_type.get_context(), context);
    /// ```
    pub fn custom_width_int_type(&self, bits: u32) -> IntType {
        unsafe { IntType::new(LLVMIntTypeInContext(self.context, bits)) }
    }

    /// Gets the `MetadataType` representing 128 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```
    /// use inkwell::context::Context;
    /// use inkwell::values::IntValue;
    ///
    /// let context = Context::create();
    /// let md_type = context.metadata_type();
    ///
    /// assert_eq!(*md_type.get_context(), context);
    /// ```
    #[llvm_versions(6.0..=latest)]
    pub fn metadata_type(&self) -> MetadataType {
        unsafe { MetadataType::new(LLVMMetadataTypeInContext(self.context)) }
    }

    /// Gets the `IntType` representing a bit width of a pointer. It will be assigned the referenced context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::OptimizationLevel;
    /// use inkwell::context::Context;
    /// use inkwell::targets::{InitializationConfig, Target};
    ///
    /// Target::initialize_native(&InitializationConfig::default()).expect("Failed to initialize native target");
    ///
    /// let context = Context::create();
    /// let module = context.create_module("sum");
    /// let execution_engine = module.create_jit_execution_engine(OptimizationLevel::None).unwrap();
    /// let target_data = execution_engine.get_target_data();
    /// let int_type = context.ptr_sized_int_type(&target_data, None);
    /// ```
    pub fn ptr_sized_int_type(&self, target_data: &TargetData, address_space: Option<AddressSpace>) -> IntType {
        let int_type_ptr = match address_space {
            Some(address_space) => unsafe {
                LLVMIntPtrTypeForASInContext(self.context, target_data.target_data, address_space as u32)
            },
            None => unsafe { LLVMIntPtrTypeInContext(self.context, target_data.target_data) },
        };

        unsafe { IntType::new(int_type_ptr) }
    }

    /// Gets the `FloatType` representing a 16 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    ///
    /// let f16_type = context.f16_type();
    ///
    /// assert_eq!(*f16_type.get_context(), context);
    /// ```
    pub fn f16_type(&self) -> FloatType {
        unsafe { FloatType::new(LLVMHalfTypeInContext(self.context)) }
    }

    /// Gets the `FloatType` representing a 32 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    ///
    /// let f32_type = context.f32_type();
    ///
    /// assert_eq!(*f32_type.get_context(), context);
    /// ```
    pub fn f32_type<'ctx>(&'ctx self) -> FloatType<'ctx> {
        unsafe { FloatType::new(LLVMFloatTypeInContext(self.context)) }
    }

    /// Gets the `FloatType` representing a 64 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    ///
    /// let f64_type = context.f64_type();
    ///
    /// assert_eq!(*f64_type.get_context(), context);
    /// ```
    pub fn f64_type(&self) -> FloatType {
        unsafe { FloatType::new(LLVMDoubleTypeInContext(self.context)) }
    }

    /// Gets the `FloatType` representing a 80 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    ///
    /// let x86_f80_type = context.x86_f80_type();
    ///
    /// assert_eq!(*x86_f80_type.get_context(), context);
    /// ```
    pub fn x86_f80_type(&self) -> FloatType {
        unsafe { FloatType::new(LLVMX86FP80TypeInContext(self.context)) }
    }

    /// Gets the `FloatType` representing a 128 bit width. It will be assigned the current context.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    ///
    /// let f128_type = context.f128_type();
    ///
    /// assert_eq!(*f128_type.get_context(), context);
    /// ```
    // IEEE 754-2008’s binary128 floats according to https://internals.rust-lang.org/t/pre-rfc-introduction-of-half-and-quadruple-precision-floats-f16-and-f128/7521
    pub fn f128_type(&self) -> FloatType {
        unsafe { FloatType::new(LLVMFP128TypeInContext(self.context)) }
    }

    /// Gets the `FloatType` representing a 128 bit width. It will be assigned the current context.
    ///
    /// PPC is two 64 bits side by side rather than one single 128 bit float.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    ///
    /// let f128_type = context.ppc_f128_type();
    ///
    /// assert_eq!(*f128_type.get_context(), context);
    /// ```
    // Two 64 bits according to https://internals.rust-lang.org/t/pre-rfc-introduction-of-half-and-quadruple-precision-floats-f16-and-f128/7521
    pub fn ppc_f128_type(&self) -> FloatType {
        unsafe { FloatType::new(LLVMPPCFP128TypeInContext(self.context)) }
    }

    /// Creates a `StructType` definiton from heterogeneous types in the current `Context`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let f32_type = context.f32_type();
    /// let i16_type = context.i16_type();
    /// let struct_type = context.struct_type(&[i16_type.into(), f32_type.into()], false);
    ///
    /// assert_eq!(struct_type.get_field_types(), &[i16_type.into(), f32_type.into()]);
    /// ```
    // REVIEW: AnyType but VoidType? FunctionType?
    pub fn struct_type(&self, field_types: &[BasicTypeEnum], packed: bool) -> StructType {
        let mut field_types: Vec<LLVMTypeRef> = field_types.iter().map(|val| val.as_type_ref()).collect();
        unsafe {
            StructType::new(LLVMStructTypeInContext(
                self.context,
                field_types.as_mut_ptr(),
                field_types.len() as u32,
                packed as i32,
            ))
        }
    }

    /// Creates an opaque `StructType` with no type definition yet defined.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let f32_type = context.f32_type();
    /// let i16_type = context.i16_type();
    /// let struct_type = context.opaque_struct_type("my_struct");
    ///
    /// assert_eq!(struct_type.get_field_types(), &[]);
    /// ```
    pub fn opaque_struct_type(&self, name: &str) -> StructType {
        let c_string = to_c_str(name);

        unsafe { StructType::new(LLVMStructCreateNamed(self.context, c_string.as_ptr())) }
    }

    /// Creates a constant `StructValue` from constant values.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let f32_type = context.f32_type();
    /// let i16_type = context.i16_type();
    /// let f32_one = f32_type.const_float(1.);
    /// let i16_two = i16_type.const_int(2, false);
    /// let const_struct = context.const_struct(&[i16_two.into(), f32_one.into()], false);
    ///
    /// assert_eq!(const_struct.get_type().get_field_types(), &[i16_type.into(), f32_type.into()]);
    /// ```
    pub fn const_struct(&self, values: &[BasicValueEnum], packed: bool) -> StructValue {
        let mut args: Vec<LLVMValueRef> = values.iter().map(|val| val.as_value_ref()).collect();
        unsafe {
            StructValue::new(LLVMConstStructInContext(
                self.context,
                args.as_mut_ptr(),
                args.len() as u32,
                packed as i32,
            ))
        }
    }

    /// Append a named `BasicBlock` at the end of the referenced `FunctionValue`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let module = context.create_module("my_mod");
    /// let void_type = context.void_type();
    /// let fn_type = void_type.fn_type(&[], false);
    /// let fn_value = module.add_function("my_fn", fn_type, None);
    /// let entry_basic_block = context.append_basic_block(fn_value, "entry");
    ///
    /// assert_eq!(fn_value.count_basic_blocks(), 1);
    ///
    /// let last_basic_block = context.append_basic_block(fn_value, "last");
    ///
    /// assert_eq!(fn_value.count_basic_blocks(), 2);
    /// assert_eq!(fn_value.get_first_basic_block().unwrap(), entry_basic_block);
    /// assert_eq!(fn_value.get_last_basic_block().unwrap(), last_basic_block);
    /// ```
    pub fn append_basic_block(&self, function: FunctionValue, name: &str) -> BasicBlock {
        let c_string = to_c_str(name);

        unsafe {
            BasicBlock::new(LLVMAppendBasicBlockInContext(
                self.context,
                function.as_value_ref(),
                c_string.as_ptr(),
            ))
            .expect("Appending basic block should never fail")
        }
    }

    /// Append a named `BasicBlock` after the referenced `BasicBlock`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let module = context.create_module("my_mod");
    /// let void_type = context.void_type();
    /// let fn_type = void_type.fn_type(&[], false);
    /// let fn_value = module.add_function("my_fn", fn_type, None);
    /// let entry_basic_block = context.append_basic_block(fn_value, "entry");
    ///
    /// assert_eq!(fn_value.count_basic_blocks(), 1);
    ///
    /// let last_basic_block = context.insert_basic_block_after(entry_basic_block, "last");
    ///
    /// assert_eq!(fn_value.count_basic_blocks(), 2);
    /// assert_eq!(fn_value.get_first_basic_block().unwrap(), entry_basic_block);
    /// assert_eq!(fn_value.get_last_basic_block().unwrap(), last_basic_block);
    /// ```
    // REVIEW: What happens when using these methods and the BasicBlock doesn't have a parent?
    // Should they be callable at all? Needs testing to see what LLVM will do, I suppose. See below unwrap.
    // Maybe need SubTypes: BasicBlock<HasParent>, BasicBlock<Orphan>?
    pub fn insert_basic_block_after(&self, basic_block: BasicBlock, name: &str) -> BasicBlock {
        match basic_block.get_next_basic_block() {
            Some(next_basic_block) => self.prepend_basic_block(next_basic_block, name),
            None => {
                let parent_fn = basic_block.get_parent().unwrap();

                self.append_basic_block(parent_fn, name)
            },
        }
    }

    /// Prepend a named `BasicBlock` before the referenced `BasicBlock`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let module = context.create_module("my_mod");
    /// let void_type = context.void_type();
    /// let fn_type = void_type.fn_type(&[], false);
    /// let fn_value = module.add_function("my_fn", fn_type, None);
    /// let entry_basic_block = context.append_basic_block(fn_value, "entry");
    ///
    /// assert_eq!(fn_value.count_basic_blocks(), 1);
    ///
    /// let first_basic_block = context.prepend_basic_block(entry_basic_block, "first");
    ///
    /// assert_eq!(fn_value.count_basic_blocks(), 2);
    /// assert_eq!(fn_value.get_first_basic_block().unwrap(), first_basic_block);
    /// assert_eq!(fn_value.get_last_basic_block().unwrap(), entry_basic_block);
    /// ```
    pub fn prepend_basic_block(&self, basic_block: BasicBlock, name: &str) -> BasicBlock {
        let c_string = to_c_str(name);

        unsafe {
            BasicBlock::new(LLVMInsertBasicBlockInContext(
                self.context,
                basic_block.basic_block,
                c_string.as_ptr(),
            ))
            .expect("Prepending basic block should never fail")
        }
    }

    /// Creates a `MetadataValue` tuple of heterogeneous types (a "Node") for the current context. It can be assigned to a value.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let i8_type = context.i8_type();
    /// let i8_two = i8_type.const_int(2, false);
    /// let f32_type = context.f32_type();
    /// let f32_zero = f32_type.const_float(0.);
    /// let md_node = context.metadata_node(&[i8_two.into(), f32_zero.into()]);
    /// let f32_one = f32_type.const_float(1.);
    /// let void_type = context.void_type();
    ///
    /// let builder = context.create_builder();
    /// let module = context.create_module("my_mod");
    /// let fn_type = void_type.fn_type(&[f32_type.into()], false);
    /// let fn_value = module.add_function("my_func", fn_type, None);
    /// let entry_block = context.append_basic_block(fn_value, "entry");
    ///
    /// builder.position_at_end(entry_block);
    ///
    /// let ret_instr = builder.build_return(None);
    ///
    /// assert!(md_node.is_node());
    ///
    /// ret_instr.set_metadata(md_node, 0);
    /// ```
    // REVIEW: Maybe more helpful to beginners to call this metadata_tuple?
    // REVIEW: Seems to be unassgned to anything
    pub fn metadata_node(&self, values: &[BasicMetadataValueEnum]) -> MetadataValue {
        let mut tuple_values: Vec<LLVMValueRef> = values.iter().map(|val| val.as_value_ref()).collect();
        unsafe {
            MetadataValue::new(LLVMMDNodeInContext(
                self.context,
                tuple_values.as_mut_ptr(),
                tuple_values.len() as u32,
            ))
        }
    }

    /// Creates a `MetadataValue` string for the current context. It can be assigned to a value.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let md_string = context.metadata_string("Floats are awesome!");
    /// let f32_type = context.f32_type();
    /// let f32_one = f32_type.const_float(1.);
    /// let void_type = context.void_type();
    ///
    /// let builder = context.create_builder();
    /// let module = context.create_module("my_mod");
    /// let fn_type = void_type.fn_type(&[f32_type.into()], false);
    /// let fn_value = module.add_function("my_func", fn_type, None);
    /// let entry_block = context.append_basic_block(fn_value, "entry");
    ///
    /// builder.position_at_end(entry_block);
    ///
    /// let ret_instr = builder.build_return(None);
    ///
    /// assert!(md_string.is_string());
    ///
    /// ret_instr.set_metadata(md_string, 0);
    /// ```
    // REVIEW: Seems to be unassigned to anything
    pub fn metadata_string(&self, string: &str) -> MetadataValue {
        let c_string = to_c_str(string);

        unsafe {
            MetadataValue::new(LLVMMDStringInContext(
                self.context,
                c_string.as_ptr(),
                string.len() as u32,
            ))
        }
    }

    /// Obtains the index of a metadata kind id. If the string doesn't exist, LLVM will add it at index `FIRST_CUSTOM_METADATA_KIND_ID` onward.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    /// use inkwell::values::FIRST_CUSTOM_METADATA_KIND_ID;
    ///
    /// let context = Context::create();
    ///
    /// assert_eq!(context.get_kind_id("dbg"), 0);
    /// assert_eq!(context.get_kind_id("tbaa"), 1);
    /// assert_eq!(context.get_kind_id("prof"), 2);
    ///
    /// // Custom kind id doesn't exist in LLVM until now:
    /// assert_eq!(context.get_kind_id("foo"), FIRST_CUSTOM_METADATA_KIND_ID);
    /// ```
    pub fn get_kind_id(&self, key: &str) -> u32 {
        unsafe { LLVMGetMDKindIDInContext(self.context, key.as_ptr() as *const ::libc::c_char, key.len() as u32) }
    }

    // LLVM 3.9+
    // pub fn get_diagnostic_handler(&self) -> DiagnosticHandler {
    //     let handler = unsafe {
    //         LLVMContextGetDiagnosticHandler(self.context)
    //     };

    //     // REVIEW: Can this be null?

    //     DiagnosticHandler::new(handler)
    // }

    /// Creates an enum `Attribute` in this `Context`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let enum_attribute = context.create_enum_attribute(0, 10);
    ///
    /// assert!(enum_attribute.is_enum());
    /// ```
    #[llvm_versions(3.9..=latest)]
    pub fn create_enum_attribute(&self, kind_id: u32, val: u64) -> Attribute {
        unsafe { Attribute::new(LLVMCreateEnumAttribute(self.context, kind_id, val)) }
    }

    /// Creates a string `Attribute` in this `Context`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    ///
    /// let context = Context::create();
    /// let string_attribute = context.create_string_attribute("my_key_123", "my_val");
    ///
    /// assert!(string_attribute.is_string());
    /// ```
    #[llvm_versions(3.9..=latest)]
    pub fn create_string_attribute(&self, key: &str, val: &str) -> Attribute {
        unsafe {
            Attribute::new(LLVMCreateStringAttribute(
                self.context,
                key.as_ptr() as *const _,
                key.len() as u32,
                val.as_ptr() as *const _,
                val.len() as u32,
            ))
        }
    }

    /// Create an enum `Attribute` with an `AnyTypeEnum` attached to it.
    ///
    /// # Example
    /// ```rust
    /// use inkwell::context::Context;
    /// use inkwell::attributes::Attribute;
    /// use inkwell::types::AnyType;
    ///
    /// let context = Context::create();
    /// let kind_id = Attribute::get_named_enum_kind_id("sret");
    /// let any_type = context.i32_type().as_any_type_enum();
    /// let type_attribute = context.create_type_attribute(
    ///     kind_id,
    ///     any_type,
    /// );
    ///
    /// assert!(type_attribute.is_type());
    /// assert_eq!(type_attribute.get_type_value(), any_type);
    /// assert_ne!(type_attribute.get_type_value(), context.i64_type().as_any_type_enum());
    /// ```
    #[llvm_versions(12.0..=latest)]
    pub fn create_type_attribute(&self, kind_id: u32, type_ref: AnyTypeEnum) -> Attribute {
        unsafe { Attribute::new(LLVMCreateTypeAttribute(self.context, kind_id, type_ref.as_type_ref())) }
    }

    /// Creates a const string which may be null terminated.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use inkwell::context::Context;
    /// use inkwell::values::AnyValue;
    ///
    /// let context = Context::create();
    /// let string = context.const_string(b"my_string", false);
    ///
    /// assert_eq!(string.print_to_string().to_string(), "[9 x i8] c\"my_string\"");
    /// ```
    // SubTypes: Should return VectorValue<IntValue<i8>>
    pub fn const_string(&self, string: &[u8], null_terminated: bool) -> VectorValue {
        unsafe {
            VectorValue::new(LLVMConstStringInContext(
                self.context,
                string.as_ptr() as *const ::libc::c_char,
                string.len() as u32,
                !null_terminated as i32,
            ))
        }
    }

    pub(crate) fn set_diagnostic_handler(
        &self,
        handler: extern "C" fn(LLVMDiagnosticInfoRef, *mut c_void),
        void_ptr: *mut c_void,
    ) {
        unsafe { LLVMContextSetDiagnosticHandler(self.context, Some(handler), void_ptr) }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            LLVMContextDispose(self.context);
        }
    }
}

/// A `ContextRef` is a smart pointer allowing borrowed access to a type's `Context`.
#[derive(Debug, PartialEq, Eq)]
pub struct ContextRef<'ctx> {
    context: ManuallyDrop<Context>,
    _marker: PhantomData<&'ctx ()>,
}

impl<'ctx> ContextRef<'ctx> {
    pub(crate) unsafe fn new(context: LLVMContextRef) -> Self {
        ContextRef {
            context: ManuallyDrop::new(Context::new(context)),
            _marker: PhantomData,
        }
    }

    // Gets a usable context object with a correct lifetime.
    // FIXME: Not safe :(
    // #[cfg(feature = "experimental")]
    // pub unsafe fn get(&self) -> &'ctx Context {
    //     // Safety: Although strictly untrue that a local reference to the context field
    //     // is guaranteed to live for the entirety of 'ctx:
    //     // 1) ContextRef cannot outlive 'ctx
    //     // 2) Any method called called with this context object will inherit 'ctx,
    //     // which is its proper lifetime and does not point into this context object
    //     // specifically but towards the actual context pointer in LLVM.
    //     &*(&*self.context as *const Context)
    // }
}

impl Deref for ContextRef<'_> {
    type Target = Context;

    fn deref(&self) -> &Self::Target {
        &*self.context
    }
}

#[cfg(feature = "internal-getters")]
impl LLVMReference<LLVMContextRef> for Context {
    unsafe fn get_ref(&self) -> LLVMContextRef {
        self.context
    }
}
