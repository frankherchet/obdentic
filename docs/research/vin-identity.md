# Offline VIN manufacturer hints

`src/vin_identity.rs` is an offline, partial lookup for the first three VIN
characters (the WMI). It checks VIN format (17 ASCII characters from the
allowed VIN alphabet, with `I`, `O`, and `Q` excluded), normalizes ASCII case,
and returns a manufacturer name for WMIs included in the local snapshot. A
make hint is included only when the corresponding vPIC `Make` value identifies
one clear make. `confidence` describes only the manufacturer match. The helper
does not check the VIN check digit, identify a model or engine, or validate an
OBD profile.

The snapshot below was checked against the official NHTSA vPIC `DecodeWMI`
records on 2026-10-23. Each linked record is the exact API query used for that
prefix; the `manufacturer` value comes from its `ManufacturerName` field.

| WMI | Manufacturer | Make hint |
| --- | --- | --- |
| [WVW](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WVW?format=json), [WVG](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WVG?format=json) | VOLKSWAGEN AG | Volkswagen |
| [WV1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WV1?format=json), [WV2](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WV2?format=json) | VOLKSWAGEN GROUP OF AMERICA, INC. | Volkswagen |
| [3VW](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/3VW?format=json) | VOLKSWAGEN DE MEXICO SA DE CV | Volkswagen |
| [WAU](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WAU?format=json), [WA1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WA1?format=json), [TRU](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/TRU?format=json) | AUDI AG | Audi |
| [WUA](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WUA?format=json) | AUDI SPORT GMBH | Audi |
| [WDB](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WDB?format=json), [WDC](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WDC?format=json), [WDD](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WDD?format=json) | MERCEDES-BENZ CARS | Mercedes-Benz for WDC/WDD; omitted for WDB because its record lists both Mercedes-Benz and Maybach |
| [WBA](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WBA?format=json), [WBY](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WBY?format=json) | BMW AG | BMW |
| [WBS](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WBS?format=json) | BMW M GMBH | BMW |
| [WMW](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WMW?format=json) | BMW AG | MINI |
| [W0L](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/W0L?format=json) | ADAM OPEL AG | Opel |
| [VF1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/VF1?format=json) | RENAULT GROUP | Omitted because the record lists multiple makes |
| [VF3](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/VF3?format=json) | AUTOMOBILES PEUGEOT | Peugeot |
| [YV1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/YV1?format=json) | VOLVO CAR CORPORATION | Volvo |
| [ZFA](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/ZFA?format=json) | STELLANTIS EUROPE SPA | Fiat |
| [ZAR](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/ZAR?format=json) | ALFA ROMEO S.P.A. | Alfa Romeo |
| [ZFF](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/ZFF?format=json) | FERRARI S.P.A. | Ferrari |
| [ZAM](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/ZAM?format=json) | MASERATI S.P.A. | Maserati |
| [JHM](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/JHM?format=json) | HONDA MOTOR CO., LTD. | Honda |
| [1HG](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/1HG?format=json) | AMERICAN HONDA MOTOR CO., INC. | Honda |
| [JTD](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/JTD?format=json) | TOYOTA MOTOR CORPORATION | Toyota |
| [4T1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/4T1?format=json) | TOYOTA MOTOR MANUFACTURING, KENTUCKY, INC. | Toyota |
| [2T1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/2T1?format=json) | TOYOTA MOTOR MANUFACTURING CANADA | Toyota |
| [JN1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/JN1?format=json) | NISSAN MOTOR COMPANY, LTD | Omitted because the record lists multiple makes |
| [JM1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/JM1?format=json) | MAZDA MOTOR CORPORATION | Mazda |
| [KMH](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/KMH?format=json) | HYUNDAI MOTOR CO | Omitted because the record lists Hyundai and Genesis |
| [KNA](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/KNA?format=json) | KIA CORPORATION | Kia |
| [JF1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/JF1?format=json) | SUBARU CORPORATION | Omitted because the record lists Toyota and Subaru |
| [4S3](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/4S3?format=json) | SUBARU OF AMERICA, INC | Subaru |
| [1G1](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/1G1?format=json) | GENERAL MOTORS LLC | Chevrolet |
| [1FT](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/1FT?format=json) | FORD MOTOR COMPANY | Ford |
| [5YJ](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/5YJ?format=json) | TESLA, INC. | Tesla |
| [WP0](https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/WP0?format=json) | DR. ING. H.C.F. PORSCHE AG | Porsche |

## Limits

- The vPIC API documentation says `DecodeWMI` accepts either three characters
  (VIN positions 1–3) or six characters (positions 1–3 and 12–14). A VIN whose
  third character is `9` uses positions 12–14 for the rest of a low-volume WMI,
  so this three-character helper does not resolve it. This rule is stated in
  the [NHTSA New Manufacturers Handbook](https://www.nhtsa.gov/sites/nhtsa.gov/files/manufacturer_information_march2014.pdf).
- NHTSA describes vPIC as manufacturer-reported data for vehicles intended for
  sale or import into the United States and warns that other vehicles can have
  limited results. A missing local entry is therefore `unknown`, not proof the
  prefix is invalid.
- The API is used only as a source for this checked-in snapshot. The helper
  makes no network calls, sends no VIN anywhere, and reports `coverage: partial`.
- vPIC returned no rows for `TMB` and `VSS` when checked. They are omitted from
  the mapping; this does not rule out those WMIs outside vPIC's coverage.

Sources: [NHTSA vPIC API documentation](https://vpic.nhtsa.dot.gov/api/) and
[NHTSA vPIC coverage and database information](https://vpic.nhtsa.dot.gov/).

## German sources checked

The [German Federal Portal's WMI service description](https://verwaltung.bund.de/leistungsverzeichnis/de/leistung/99026008069000)
confirms that the KBA assigns WMIs to manufacturers whose administrative
headquarters are in Germany. It describes an application and assignment
process, not a public lookup table of existing WMI-to-manufacturer records, so
it was not used to seed the local mapping.

The KBA's official [passenger-car manufacturer and trade-name dataset](https://www.govdata.de/suche/daten/fz-hersteller-handelsnamen-pkw)
and [commercial-vehicle manufacturer and trade-name dataset](https://www.govdata.de/suche/daten/fz-hersteller-handelsnamen-nfz)
are published through GovData. Their descriptions say they resolve vehicle
stock using the HSN (manufacturer key) and TSN (type key) printed on German
registration documents. HSN/TSN are not the VIN's first-three-character WMI,
so these datasets may help when those registration keys are available but do
not directly validate this WMI lookup. No KBA file was downloaded or treated as
a WMI source.
