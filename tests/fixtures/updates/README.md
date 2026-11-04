# Synthetic update verification

`payload.bin` is non-executable test data signed for version `0.2.0` by the Tauri CLI. `public.pub` verifies it; `other-public.pub` is a different public key. The temporary private keys were deleted after signing and are not release keys.

Native tests use Tauri's actual verifier, including signed-version binding, without installing or launching the payload. These fixtures must never configure production updates.

`legacy.bin.sig` signs the same bytes without a version binding and must be rejected when `requireSignedVersion` is enabled.
