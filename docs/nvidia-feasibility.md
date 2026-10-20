# Read-only NVIDIA qualification boundary

## What is queried

The adapter enumerates every NVML device by index, preserving independent name,
UUID and PCI bus ID readings. It queries driver/NVML/CUDA driver versions,
temperature (degC), graphics/memory clocks (MHz), GPU/memory utilization (%),
power usage (mW), power-limit current/default/min/max (mW), and current/min/max
graphics and memory clock offsets (MHz). The CUDA driver version uses NVIDIA's
encoded integer, not a CUDA toolkit version. `header_api_version: 13` is the ABI
source version, not a claim that the installed runtime implements API 13.
Memory utilization is the percentage of the sampling period during which the
memory interface was reading or writing device memory, not allocated VRAM
as a percentage of capacity.

Legacy `graphics_clock_offset` / `memory_clock_offset` records retain documented
`nvmlDeviceGetGpcClkVfOffset`, `nvmlDeviceGetMemClkVfOffset` and corresponding
`MinMaxVfOffset` results. Separately, `graphics_clock_offset_p0` and
`memory_clock_offset_p0` use the newer documented `nvmlDeviceGetClockOffsets`
getter, with graphics domain 0 or memory domain 2 and **P0 only**. The P0 query
does not force the GPU into P0 or test other performance states. Legacy failure
does not suppress the modern query; modern failure never overwrites legacy
evidence. These are additive control-map entries in report schema 1.

Modern reads use NVIDIA's 24-byte `nvmlClockOffset_v1_t`, with struct version
`24 | (1 << 24)`. Input identity and returned offsets/bounds are validated.
All output integers start as invalid sentinels, not fabricated zero readings.
Missing exports and vendor errors retain the same typed failures as other
queries. Reported bounds are API-returned bounds, **not validated safe ranges**.

## Safety and interpretation

All NVML FFI lives in `src/nvml.rs`. The library remains alive through every
query; successful `nvmlInit_v2` is paired with `nvmlShutdown`, which only releases
client resources. Shutdown is **not** a GPU/driver reset. Export addresses of the
four setters are inspected only as opaque data pointers, never callable bindings.
No setter/reset/elevation/workload or `nvidia-smi` execution path exists.
Collection is a sequential, non-atomic snapshot: telemetry or external tuning
software may change values between getters. This probe does not lock other tools.

NVML is loaded only from the absolute directory returned by Windows
`GetSystemDirectoryW`, with `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR` and
`LOAD_LIBRARY_SEARCH_SYSTEM32`. PATH, current directory, environment-supplied
DLL roots and user arguments are never library search sources. This trusts the
OS/driver installation, not arbitrary DLLs; it does not independently authenticate
the driver's signature. Older installations only providing NVML under Program
Files/NVSMI are intentionally not searched and yield `runtime_missing` here.

Every return code is checked before output buffers are read. Numeric validation
rejects unavailable sentinels, inverted bounds, invalid utilization and implausible
readings; the conservative guardrails are not advertised hardware specifications.
A zero may be a valid utilization, offset or power sample, never a substitute for
a failed getter. No retries, stress testing or concurrent tuning are performed.
An incomplete report preserves all successful fields. A missing per-device handle
is represented as a failed device record, never fabricated measurements.

Successful getter access or a present setter export **does not prove writable
support**, permissions, recoverability, undervolting or V/F curve access. Missing
bounds, unreliable readback, unsupported or denied fields block later qualification.
No feature is reported as a supported tuning control.

## Future protocol — requires separate informed consent

This protocol is documentation only and is not executable in this milestone:

1. Identify the target by UUID and PCI identity, not assumed index zero; reject
   ambiguous identity or another active tuning owner.
2. Obtain explicit approval for the specific control, bounds, risks and duration.
   Capture known-good original values, trustworthy getters and documented restore
   semantics first. Missing bounds/readback/restore guarantees mean stop.
3. Only in a separately reviewed write-capable tool, make one bounded reversible
   change. Read back immediately, compare expected values and watch failures.
4. Restore the exact original value and verify readback on success, failure,
   cancellation and orderly shutdown. A crash/reboot recovery plan and explicit
   consent are prerequisites; never assume cleanup always runs.
5. Record per-device write/readback/restore evidence; any failed restoration
   invalidates qualification. Stress/workload testing needs its own consent.

## Sources and ABI

- [NVIDIA NVML API reference](https://docs.nvidia.com/deploy/nvml-api/)
- [Documented clock-offset getter](https://docs.nvidia.com/deploy/nvml-api/latest/api/group__nvmlDeviceQueries.html),
  checked 2026-10-15. The pinned header below defines the structure and version macro.
- [NVIDIA maintained go-nvml header](https://github.com/NVIDIA/go-nvml/blob/6b38eea7ed6854b19001f44231e9385661e90c46/pkg/nvml/nvml.h),
  observed version **13.4.92**, API **13**, on 2026-10-15. Published C signatures
  are represented as minimal Rust FFI aliases and `repr(C)` structs. PCI v3 is
  68 bytes; utilization is two 32-bit unsigned integers. No NVIDIA binary or
  full header is redistributed.
- [Windows LoadLibraryExW search flags](https://learn.microsoft.com/windows/win32/api/libloaderapi/nf-libloaderapi-loadlibraryexw)
- [Windows GetSystemDirectoryW](https://learn.microsoft.com/windows/win32/api/sysinfoapi/nf-sysinfoapi-getsystemdirectoryw)

## Verification record

On **2026-10-15 at 19:23:19 UTC**, the unelevated live probe completed on the
NVIDIA GeForce RTX 2070 SUPER with driver **617.42**. Modern P0 queries returned:

| Domain | Current offset | API minimum | API maximum |
|--------|---------------:|------------:|------------:|
| Graphics | 0 MHz | -1000 MHz | +1000 MHz |
| Memory | 0 MHz | -2000 MHz | +6000 MHz |

These are raw NVML MHz values, not Afterburner slider-unit conversions or safe
candidate ranges. The legacy offset getters still returned status 3
(`NOT_SUPPORTED`). The modern setter export was present but never called;
write verification remains `not_tested`. No settings were changed and no
restoration, undervolting or other P-state behavior was tested.

See the implementation spec's Verification section for actual execution results.
Portable fixtures cover all matrix rows without physical hardware. Non-Windows
live code is a structured unsupported-platform result; running that branch needs
a non-Windows host. Source-level mutation checks are regression checks, not a
formal proof against arbitrary future code or compromised dependencies/drivers.
