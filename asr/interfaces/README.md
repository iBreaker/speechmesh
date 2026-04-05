# ASR Interfaces

The common ASR contract should stay small.

Minimum concerns:
- transcribe file or buffer
- start stream
- receive partial results
- receive final results
- stop or cancel session
- declare capabilities

Do not force advanced features into the base contract. Expose them through capability flags or provider-specific options.
