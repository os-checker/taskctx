use core::{arch::naked_asm, fmt};
use memory_addr::VirtAddr;

#[repr(C)]
#[derive(Debug, Default)]
struct ContextSwitchFrame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbx: u64,
    rbp: u64,
    rip: u64,
}

/// A 512-byte memory region for the FXSAVE/FXRSTOR instruction to save and
/// restore the x87 FPU, MMX, XMM, and MXCSR registers.
///
/// See <https://www.felixcloutier.com/x86/fxsave> for more details.
#[allow(missing_docs)]
#[repr(C, align(16))]
#[derive(Debug)]
pub struct FxsaveArea {
    pub fcw: u16,
    pub fsw: u16,
    pub ftw: u16,
    pub fop: u16,
    pub fip: u64,
    pub fdp: u64,
    pub mxcsr: u32,
    pub mxcsr_mask: u32,
    pub st: [u64; 16],
    pub xmm: [u64; 32],
    _padding: [u64; 12],
}

static_assertions::const_assert_eq!(core::mem::size_of::<FxsaveArea>(), 512);

/// Extended state of a task, such as FP/SIMD states.
pub struct ExtendedState {
    /// Memory region for the FXSAVE/FXRSTOR instruction.
    pub fxsave_area: FxsaveArea,
}

#[cfg(feature = "fp_simd")]
impl ExtendedState {
    /// Saves the FP/SIMD states to the memory region.
    #[inline]
    pub fn save(&mut self) {
        unsafe { core::arch::x86_64::_fxsave64(&mut self.fxsave_area as *mut _ as *mut u8) }
    }

    /// Restores the FP/SIMD states from the memory region.
    #[inline]
    pub fn restore(&self) {
        unsafe { core::arch::x86_64::_fxrstor64(&self.fxsave_area as *const _ as *const u8) }
    }

    const fn default() -> Self {
        let mut area: FxsaveArea = unsafe { core::mem::MaybeUninit::zeroed().assume_init() };
        area.fcw = 0x37f;
        area.ftw = 0xffff;
        area.mxcsr = 0x1f80;
        Self { fxsave_area: area }
    }
}

impl fmt::Debug for ExtendedState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ExtendedState")
            .field("fxsave_area", &self.fxsave_area)
            .finish()
    }
}

/// Saved hardware states of a task.
///
/// The context usually includes:
///
/// - Callee-saved registers
/// - Stack pointer register
/// - Thread pointer register (for thread-local storage, currently unsupported)
/// - FP/SIMD registers
///
/// On context switch, current task saves its context from CPU to memory,
/// and the next task restores its context from memory to CPU.
///
/// On x86_64, callee-saved registers are saved to the kernel stack by the
/// `PUSH` instruction. So that [`rsp`] is the `RSP` after callee-saved
/// registers are pushed, and [`kstack_top`] is the top of the kernel stack
/// (`RSP` before any push).
///
/// [`rsp`]: TaskContext::rsp
/// [`kstack_top`]: TaskContext::kstack_top
#[derive(Debug)]
pub struct TaskContext {
    /// The kernel stack top of the task.
    pub kstack_top: VirtAddr,
    /// `RSP` after all callee-saved registers are pushed.
    pub rsp: u64,
    /// Thread Local Storage (TLS).
    pub fs_base: usize,
    /// Extended states, i.e., FP/SIMD states.
    #[cfg(feature = "fp_simd")]
    pub ext_state: ExtendedState,
}

impl Default for TaskContext {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskContext {
    /// Creates a new default context for a new task.
    pub const fn new() -> Self {
        Self {
            kstack_top: VirtAddr::from(0),
            rsp: 0,
            fs_base: 0,
            #[cfg(feature = "fp_simd")]
            ext_state: ExtendedState::default(),
        }
    }

    /// Initializes the context for a new task, with the given entry point and
    /// kernel stack.
    pub fn init(&mut self, entry: usize, kstack_top: VirtAddr, tls_area: VirtAddr) {
        unsafe {
            // x86_64 calling convention: the stack must be 16-byte aligned before
            // calling a function. That means when entering a new task (`ret` in `context_switch`
            // is executed), (stack pointer + 8) should be 16-byte aligned.
            let frame_ptr = (kstack_top.as_mut_ptr() as *mut u64).sub(1);
            let rbp = frame_ptr as u64;
            let frame_ptr = (frame_ptr as *mut ContextSwitchFrame).sub(1);
            core::ptr::write(frame_ptr, ContextSwitchFrame {
                rip: entry as _,
                rbp,
                ..Default::default()
            });
            self.rsp = frame_ptr as u64;
        }
        self.kstack_top = kstack_top;
        self.fs_base = tls_area.as_usize();
    }

    pub fn thread_saved_fp(&self) -> usize {
        let frame_ptr = self.rsp as *const ContextSwitchFrame;
        unsafe { (*frame_ptr).rbp as usize }
    }

    pub fn thread_saved_pc(&self) -> usize {
        let frame_ptr = self.rsp as *const ContextSwitchFrame;
        unsafe { (*frame_ptr).rip as usize }
    }
}

#[naked]
/// Switches the context from the current task to the next task.
///
/// # Safety
///
/// This function is unsafe because it directly manipulates the CPU registers.
pub unsafe extern "C" fn context_switch(_current_stack: &mut u64, _next_stack: &u64) {
    unsafe {
        naked_asm!(
            "
        push    rbp
        push    rbx
        push    r12
        push    r13
        push    r14
        push    r15
        mov     [rdi], rsp

        mov     rsp, [rsi]
        pop     r15
        pop     r14
        pop     r13
        pop     r12
        pop     rbx
        pop     rbp
        ret",
        )
    }
}
