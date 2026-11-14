# 0005. ChatGPT plan access for the Android assistant
Date: 2026-11-14
Status: proposed

## Context
Epic #317 and spike #324 investigate whether a native Android client can use a user's ChatGPT plan for streamed, function-calling diagnostic assistance without a backend. The prototype used only synthetic vehicle data; it did not connect to an adapter or vehicle. The real phone ran Android 16, SDK 36, with Chrome (`com.android.chrome`) as its default browser.

OpenAI's [open-source direct-flow overview](https://developers.openai.com/siwc/token-sharing-open-source), [sign-in contract](https://developers.openai.com/siwc/token-sharing-open-source/sign-in), [models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference), [token reference](https://developers.openai.com/siwc/token-sharing-open-source/token-reference), [errors](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery), [preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations), [UI guidelines](https://developers.openai.com/siwc/ui-ux-guidelines) and [function-calling guide](https://developers.openai.com/api/docs/guides/function-calling) describe dynamic registration, loopback OAuth with PKCE, rotating refresh tokens and streamed Responses with namespaced function tools. The direct flow requires no partner API key or client secret. Responses are sent with `store:false` and `stream:true`, and history is resent in `input` rather than via `previous_response_id`.

| Check | Result | Sanitized observation |
|---|---|---|
| S1 host identity | PASS | Persisted before sign-in; reused. |
| S2 consent | PASS | User observed consent mentioning ChatGPT-plan permission. |
| S3 callback | PASS | Variant A, loopback port 1455; one initial attempt timed out, later callbacks succeeded. The user saw the callback page; return-link operation was not confirmed. |
| S4 exchange | PASS | HTTP 200; plan scope, refresh token, ID token and earliest refresh time present. |
| S5 ID token | PASS | RS256 signature, issuer, audience, expiry and nonce validated; returning sign-in also matched subject. |
| S6 storage | PASS | Record decrypted after force-stop; refresh token present. |
| S7 model discovery | PASS | 10 listed models; selected `gpt-6.1-sol`. |
| S8 simple response | FAIL | Completed responses did not contain the exact synthetic marker `Hello, world!` on two attempts (4514 ms and 3451 ms). The response body was not logged. |
| S9 function call | PASS | `vehicle_info`, namespace field present; two requests, all output items echoed; final synthetic vehicle name matched. |
| S10 bounded diagnosis | PASS | Two requests, five calls to three distinct tools, 9587 ms; 559 input and 194 output tokens. |
| S11 injection measurement | INFO | Zero invalid and zero unknown calls across the measured runs; injected-note rerun passed. This is not proof of prompt-injection resistance. |
| S12 refresh | FAIL | HTTP 200, refresh token rotated and plan scope present, but the required subsequent S8 marker check failed. |
| S13 sign-out | PASS | Revocation HTTP 200; reuse returned HTTP 400 `invalid_grant`; local tokens deleted. |
| S14 returning sign-in | PASS | Account selection appeared; callback, ID-token validation and subject match succeeded. |

The loopback listener worked in the process (variant A). Initially the browser foregrounded the app into a background network-restricted state: the callback arrived but token exchange failed with `UnknownHostException`, despite shell DNS working. A prototype foreground `dataSync` service kept network access available; S4 and later network calls then succeeded. This adds foreground-service permissions and lifecycle considerations; it is not the plan's variant B listener service. No namespace, strict-schema or output-echo contingency was needed. The alternative `additional_tools` shape was not implemented: it was not found in the current function-calling guide. The first S8 request's shared instructions say to use only tools and answer in German, while its user prompt requests an English exact marker without tools. This conflict is a plausible explanation for the mismatch, not a measured cause; response text was deliberately not retained.

Usage-limit errors, network loss during a stream, multiple accounts and Codex app-server were not tested. No screenshots, account identifiers, credentials or vehicle evidence were retained in this report.

## Options considered
1. Sign in with ChatGPT's open-source direct flow inside the app: system browser, loopback callback, platform HTTP and a Keystore-encrypted credential file. The measured prototype also needed a foreground `dataSync` service for network access on this phone.
2. A user-supplied separately billed API key. Rejected by the maintainer's decision of 2026-11-14.
3. No cloud assistant for now; continue semantic and offline groundwork independently.

## Decision
**Proposed NO-GO for cloud integration under the spike's predeclared acceptance rule:** option 3. S8 failed its exact-marker criterion twice, so the required S3/S4/S5/S8/S9 all-pass result was not achieved. This is a failure of the specified test, not evidence that all streamed inference or function calling is unavailable: S9 and S10 completed successfully. Retry the integration decision when an agreed, non-conflicting simple-response test completes with its observable marker and the refresh test passes against it; measure return-link behavior separately. #327, #328 and #329 remain blocked. #325 and #326 can continue as groundwork for #19 and #20 using a scripted model in tests.

Any later cloud integration must be explicit opt-in, without an API-key fallback. The application-side closed validator and policy, not model instructions or tool schemas, are the safety boundary. S11's zero rejected calls in this small synthetic sample cannot establish safety.

## Consequences
- A later implementation would add the app's first `INTERNET` permission and network code. Add the permission as a single-line `<uses-permission android:name="android.permission.INTERNET" />` tag: `tools/verify-release-apk.sh` compares APK permissions for equality with single-line manifest declarations. The foreground service also requires its Android foreground-service permissions; production needs a reviewed lifecycle and user-visible notification design.
- The platform-only prototype built without new Gradle dependencies. Credentials were encrypted with an Android Keystore key, removed locally at sign-out, and removed with the prototype on uninstall. Production must protect, revoke and remove credentials with explicit error handling. Refresh tokens have a documented 30-day lifetime; expiry requires sign-in again.
- ChatGPT Plus's five-hour usage limit is shared across apps. Required UI includes **Continue with ChatGPT**, a one-time “You're using your ChatGPT plan” confirmation, a “Using ChatGPT plan” indicator and a “Manage usage” link to https://chatgpt.com/settings/usage.
- Conversation history must be resent with every request. #328 must minimize, preview and obtain consent for transmitted facts; no raw capture, VIN, ECU serial or adapter identifier is sent by default.
- The ChatGPT-plan route does not support audio input or transcription. #329 must use Android speech services and consider their privacy and connectivity properties separately.
- The repository currently has no license, although the direct flow targets open-source clients. The maintainer must choose a license before #327.
- Proposed text for a new subsection of `docs/project-goals.md` section 7, to apply in #327 only after this ADR is accepted: “Cloud inference and account use are optional and require explicit user consent. Local diagnostics, recording and replay remain usable without an account or network. Before sending diagnostic facts, show what will leave the device and minimize it; never send raw recordings or vehicle and device identifiers by default. Network access or AI output cannot elevate vehicle capabilities.” No project-goals change is made by this ADR.
- For #327, this measurement identifies a possible OAuth, storage and network-permission route but does not approve its production flow or resolve foreground-service UX. For #325, tool validation remains a closed application responsibility; tool placement and budgets remain open. For #328, history resending and explicit disclosure are required; concrete data minimization and loop budgets remain open.
