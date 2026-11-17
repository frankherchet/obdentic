# 0005. ChatGPT plan access for the Android assistant
Date: 2026-11-14, updated 2026-11-17 with the S8/S12 rerun
Status: proposed

## Context
Epic #317 and spike #324 investigate whether a native Android client can use a user's ChatGPT plan for streamed, function-calling diagnostic assistance without a backend. The prototype used only synthetic vehicle data; it did not connect to an adapter or vehicle. The real phone ran Android 16, SDK 36, with Chrome (`com.android.chrome`) as its default browser.

OpenAI's [open-source direct-flow overview](https://developers.openai.com/siwc/token-sharing-open-source), [sign-in contract](https://developers.openai.com/siwc/token-sharing-open-source/sign-in), [models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference), [token reference](https://developers.openai.com/siwc/token-sharing-open-source/token-reference), [errors](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery), [preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations), [UI guidelines](https://developers.openai.com/siwc/ui-ux-guidelines) and [function-calling guide](https://developers.openai.com/api/docs/guides/function-calling) describe dynamic registration, loopback OAuth with PKCE, rotating refresh tokens and streamed Responses with namespaced function tools. The direct flow requires no partner API key or client secret. Responses are sent with `store:false` and `stream:true`, and history is resent in `input` rather than via `previous_response_id`.

| Check | Result | Sanitized observation |
|---|---|---|
| S1 host identity | PASS | Persisted before sign-in; reused. |
| S2 consent | PASS | User observed consent mentioning ChatGPT-plan permission. |
| S3 callback | PASS | Variant A, loopback port 1455; on 2026-11-14 one initial attempt timed out, later callbacks succeeded. Passed again on 2026-11-17 (consent completed about 50 s after launch). The user saw the callback page on 2026-11-14; return-link operation was **not measured** (on 2026-11-17 the user did not tap the back link; only an `adb` VIEW intent-filter resolution was seen, which does not show browser return). |
| S4 exchange | PASS | HTTP 200; plan scope, refresh token, ID token and earliest refresh time present. Passed again on 2026-11-17. |
| S5 ID token | PASS | RS256 signature, issuer, audience, expiry and nonce validated; returning sign-in also matched subject. Passed again on 2026-11-17. |
| S6 storage | PASS | Record decrypted after force-stop; refresh token present. |
| S7 model discovery | PASS | 10 listed models; selected `gpt-6.1-sol`. Passed again on 2026-11-17. |
| S8 simple response | PASS (rerun) | 2026-11-14: FAIL, completed responses lacked the exact synthetic marker `Hello, world!` on two attempts (4514 ms and 3451 ms) because the first test's instructions conflicted with its user prompt (see below). 2026-11-17 rerun with a non-conflicting prompt (instructions and input both `Reply with exactly the text: Hello, world!`, no tools, `stream:true`, `store:false`): marker present twice (2443 ms and 4379 ms). The response body was not logged. |
| S9 function call | PASS | `vehicle_info`, namespace field present; two requests, all output items echoed; final synthetic vehicle name matched. |
| S10 bounded diagnosis | PASS | Two requests, five calls to three distinct tools, 9587 ms; 559 input and 194 output tokens. |
| S11 injection measurement | INFO | Zero invalid and zero unknown calls across the measured runs; injected-note rerun passed. This is not proof of prompt-injection resistance. |
| S12 refresh | PASS (rerun) | 2026-11-14: FAIL only because the required subsequent S8 marker check failed. 2026-11-17: refresh HTTP 200, refresh token rotated, plan scope present, and S8 on the refreshed credentials passed (3041 ms). |
| S13 sign-out | PASS | Revocation HTTP 200; reuse returned HTTP 400 `invalid_grant`; local tokens deleted. Passed again on 2026-11-17. |
| S14 returning sign-in | PASS | Account selection appeared; callback, ID-token validation and subject match succeeded. |

The loopback listener worked in the process (variant A). On 2026-11-14 the browser initially foregrounded the app into a background network-restricted state: the callback arrived but token exchange failed with `UnknownHostException`, despite shell DNS working. A prototype foreground `dataSync` service kept network access available; S4 and later network calls then succeeded. On 2026-11-17 no `UnknownHostException` occurred, but the foreground `dataSync` service was still active, so whether it is necessary is unresolved. This adds foreground-service permissions and lifecycle considerations; it is not the plan's variant B listener service. No namespace, strict-schema or output-echo contingency was needed. The alternative `additional_tools` shape was not implemented: it was not found in the current function-calling guide. The first S8 request's shared instructions say to use only tools and answer in German, while its user prompt requests an English exact marker without tools. The 2026-11-17 rerun with a non-conflicting prompt passed, which is consistent with that conflict explaining the first failure; the cause was not measured directly and response text was deliberately not retained.

The rerun of 2026-11-17 used the real phone (Mi 9T, Android 16) and covered S3/S4/S5/S7, S8, S12 and S13. S9, S10 and S11 were not rerun; they passed on 2026-11-14.

Usage-limit errors, network loss during a stream, multiple accounts and Codex app-server were not tested. No screenshots, account identifiers, credentials or vehicle evidence were retained in this report.

## Options considered
1. Sign in with ChatGPT's open-source direct flow inside the app: system browser, loopback callback, platform HTTP and a Keystore-encrypted credential file. The prototype ran with a foreground `dataSync` service on this phone; whether it is required is unresolved.
2. A user-supplied separately billed API key. Rejected by the maintainer's decision of 2026-11-14.
3. No cloud assistant for now; continue semantic and offline groundwork independently.

## Decision
**Proposed GO** for an opt-in ChatGPT-plan cloud integration (option 1) under the spike's predeclared acceptance rule: S3, S4, S5, S8, S9 and S10 all pass. S8 failed on 2026-11-14 because the test's instructions conflicted with its prompt; it passed twice on 2026-11-17 with a non-conflicting prompt, and S12 (refresh and subsequent S8) also passed. This is a proposal: the status stays `proposed` until the maintainer accepts it.

Open before #327: the maintainer must confirm OpenAI open-source-client eligibility and the SIWC/DevKit terms for this app; neither is confirmed by this ADR. #327, #328 and #329 are unblocked only after the maintainer accepts this ADR and that eligibility is confirmed. Return-link behavior (not measured) and the need for the foreground `dataSync` service must be settled in #327. #325 and #326 can continue as groundwork for #19 and #20 using a scripted model in tests.

Any cloud integration must be explicit opt-in, without an API-key fallback. The application-side closed validator and policy, not model instructions or tool schemas, are the safety boundary. S11's zero rejected calls in this small synthetic sample is not proof of injection resistance. Usage-limit errors, network loss during a stream and multiple accounts remain untested.

## Consequences
- A later implementation would add the app's first `INTERNET` permission and network code. Add the permission as a single-line `<uses-permission android:name="android.permission.INTERNET" />` tag: `tools/verify-release-apk.sh` compares APK permissions for equality with single-line manifest declarations. The foreground service also requires its Android foreground-service permissions; production needs a reviewed lifecycle and user-visible notification design.
- The platform-only prototype built without new Gradle dependencies. Credentials were encrypted with an Android Keystore key, removed locally at sign-out, and removed with the prototype on uninstall. Production must protect, revoke and remove credentials with explicit error handling. Refresh tokens have a documented 30-day lifetime; expiry requires sign-in again.
- ChatGPT Plus's five-hour usage limit is shared across apps. Required UI includes **Continue with ChatGPT**, a one-time “You're using your ChatGPT plan” confirmation, a “Using ChatGPT plan” indicator and a “Manage usage” link to https://chatgpt.com/settings/usage.
- Conversation history must be resent with every request. #328 must minimize, preview and obtain consent for transmitted facts; no raw capture, VIN, ECU serial or adapter identifier is sent by default.
- The ChatGPT-plan route does not support audio input or transcription. #329 must use Android speech services and consider their privacy and connectivity properties separately.
- The repository license is now Apache-2.0 (PR #337, merged to `main`), which resolves the earlier no-license concern. OpenAI open-source-client eligibility and the SIWC/DevKit terms still need the maintainer's confirmation before #327.
- Proposed text for a new subsection of `docs/project-goals.md` section 7, to apply in #327 only after this ADR is accepted: “Cloud inference and account use are optional and require explicit user consent. Local diagnostics, recording and replay remain usable without an account or network. Before sending diagnostic facts, show what will leave the device and minimize it; never send raw recordings or vehicle and device identifiers by default. Network access or AI output cannot elevate vehicle capabilities.” No project-goals change is made by this ADR.
- For #327, this measurement identifies a possible OAuth, storage and network-permission route but does not approve its production flow or resolve foreground-service UX. For #325, tool validation remains a closed application responsibility; tool placement and budgets remain open. For #328, history resending and explicit disclosure are required; concrete data minimization and loop budgets remain open.
