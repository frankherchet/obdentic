# Standard Mode 06 monitor results

Status: deferred after evidence review (2026-08-28).

Mode 06 is read-only, but OBDentic must not expose a generic `monitor.results`
job yet. The public evidence confirms that it carries stored non-continuous
monitor test results, but does not provide a sufficient universal decoder for
the current CAN-era J1979 path.

## What is established

- [SAE J1979](https://saemobilus.sae.org/standards/j1979_201702-e-e-diagnostic-test-modes)
  defines the emissions-related diagnostic service family and describes Mode 06
  as access to non-continuous-monitor results.
- [CARB OBD II regulation](https://ww2.arb.ca.gov/sites/default/files/barcu/regact/obd02/fro1968-2.pdf)
  §(f)(4.5) requires stored test values and limits for ISO 15765-4 vehicles,
  and allows uncompleted monitors to report zero values.
- [GM's public CAN Mode 06 definitions](https://gsitlc.ext.gm.com/gmspo/mode6/pdf/GM%20CAN%20mode%20%2406%20data%20final_dm.pdf)
  demonstrate that the meaning, unit, resolution and range depend on the
  particular OBDMID/TID/UASID combination.

## Why no decoder is added

The public material available to this project does not establish all of the
following for classic J1979 Mode 06:

- exact bounded request variants and response lengths;
- a universal OBDMID/TID/UASID registry and scaling rules;
- reliable distinction among unsupported data, `NO DATA`, and uncompleted
  monitors; and
- a Touran/VW mapping from test IDs to physical units and limits.

The missing metadata is normally supplied by the licensed
[SAE J1979 Digital Annex](https://saemobilus.sae.org/standards/j1979da_202508)
or OEM material. Inferring it from another implementation or from plausible
numbers would violate OBDentic's evidence and deterministic-decoding rules.

## Decision and future gate

No Mode 06 request, MID/TID probe, decoder, CLI command, or semantic result is
added. Existing raw capture evidence remains the only acceptable observer until
the gate is met.

A future implementation must first have the complete standards/OEM metadata for
its exact bounded request scope. It must then preserve responder identity and
the `(OBDMID, TID, UASID)` evidence, define a test-specific unit and scaling,
and use synthetic plus owned-hardware fixtures. J1979-2's different UDS service
path is a separate protocol decision and must not be silently substituted for
the current classic J1979 transport.
