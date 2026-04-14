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
b src/eval.rs:489

# schedule
b src/concurrency/thread.rs:849

b miri::borrow_tracker::AllocState::before_memory_deallocation

# b src/shims/os_str.rs:76
# b src/alloc_addresses/mod.rs:739
# b src/mirch/physical_mem.rs:185
# b src/alloc_addresses/mod.rs:741
