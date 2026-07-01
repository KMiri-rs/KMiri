# b miri::main
# b src/bin/miri.rs:754
# b miri::eval::create_ecx

# eval_entry
# b src/eval.rs:486

# adjust_alloc_root_pointer
# b src/alloc_addresses/mod.rs:617
#
# b miri::alloc_addresses::EvalContextExt::addr_from_alloc_id
# b src/alloc_addresses/mod.rs:250

# eval_entry
b src/eval.rs:472

# start_regular_thread
# b *0x555555a8c9e0

# b miri::concurrency::thread::ThreadManager::schedule_switch_thread_and_cpu -> print switch to task
b src/concurrency/thread.rs:769

# step<miri::machine::MiriMachine> 
# let old_frames = self.frame_idx();
b *0x0000555555a89d42

# rustc_const_eval/src/interpret/memory.rs get_alloc_raw_mut
# b *0x555555a67034

# let (a, ()) = self.get_ptr_alloc_mut(mplace.ptr(), size).and(misalign_res)?;
b /home/zjp/.rustup/toolchains/nightly-2026-04-03-x86_64-unknown-linux-gnu/lib/rustlib/rustc-src/rust/compiler/rustc_const_eval/src/interpret/place.rs:504

# rustc_const_eval/src/interpret/memory.rs check_and_deref_ptr
# b *0x555555a6703d

# rustc_const_eval/src/interpret/memory.rs:900 get_ptr_alloc_mut
# hbreak *0x0000555555b1d1fe
b /home/zjp/.rustup/toolchains/nightly-2026-04-03-x86_64-unknown-linux-gnu/lib/rustlib/rustc-src/rust/compiler/rustc_const_eval/src/interpret/memory.rs:900

b src/alloc_addresses/mod.rs:395

# schedule
# b src/concurrency/thread.rs:849

# b miri::borrow_tracker::AllocState::before_memory_deallocation

# b src/shims/os_str.rs:76
# b src/alloc_addresses/mod.rs:739
# b src/mirch/physical_mem.rs:185
# b src/alloc_addresses/mod.rs:741
