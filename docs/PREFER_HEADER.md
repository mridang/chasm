# `Prefer` header reference

`chasm-server` accepts per-request overrides via the standard `Prefer` HTTP
header (RFC 7240) and through `__`-prefixed query parameters. Both are
recognised by `chasm-engine` itself; the server just plumbs them through.

## Directives

| Directive | Query | Type | Default | Effect |
| --- | --- | --- | --- | --- |
| `code` | `__code` | `u16` | unset | Forces the response status code. The engine looks up an exact match in the operation's `responses` map first, then a matching `1XX`/`2XX`/`3XX`/`4XX`/`5XX` range key, then the `default` response. |
| `example` | `__example` | string | unset | Selects a named entry from the chosen response's `examples` map. Resolves `#/components/examples/X` references. |
| `dynamic` | `__dynamic` | bool | server default | When `true`, skips the example pipeline and generates the body from the response schema via `chasm-faker`. When `false`, prefers examples even if the server was started with `--dynamic`. |
| `seed` | `__seed` | `u64` | server default (`--seed`) | Seeds dynamic generation so that responses are deterministic. Has no effect when the example pipeline serves the response. |
| `validate` | `__validate` | bool | server default (`--errors`) | When `false`, skips request validation for this request even when the server was started with `--errors`. When `true`, forces validation on (no-op when already on). Provides a request-header bypass without polluting the HTTP namespace. |
| `security` | `__security` | bool | `true` | When `false`, skips the security scheme check for this request, allowing requests through that would otherwise be `401`. When `true`, forces the check on (default). |

Boolean values accept `true`/`false`, `1`/`0`, `yes`/`no`, case-insensitive.

## Parsing rules

- The `Prefer` header is a comma-separated list of `name=value` tokens, each
  trimmed of surrounding whitespace. Unknown tokens are silently ignored.
- Token names are matched case-INsensitively per RFC 7240 §2 against `code=`,
  `example=`, `dynamic=`, `seed=`, `validate=`, and `security=`.
- The header name itself (`Prefer`) is matched case-insensitively.
- Query parameter names (`__code`, `__example`, `__dynamic`, `__seed`,
  `__validate`, `__security`) are matched exactly.
- Malformed numeric values (`code=foo`, `seed=NaN`) are silently dropped and
  the directive falls back to the server default.
- Per RFC 7240 §2, tokens may carry `;`-separated parameters
  (e.g. `respond-async; wait=10`). None of chasm's documented directives
  define parameters, so anything after a `;` in a value is accepted and
  silently dropped: `Prefer: dynamic=true; foo=bar` parses as
  `dynamic=true`.
- Multiple `Prefer` headers on a single request are folded into one
  comma-separated value before parsing, matching RFC 7230 §3.2.2. Senders
  may equivalently combine them into a single header with comma separation.

## Precedence

When the same directive appears in both the header and the query string, the
**query parameter wins** — the query string is treated as the more explicit
signal.

Per-request directives are merged on top of the server-wide `MockConfig`
created from CLI flags. A directive that is unset on the request preserves
the server default.

## Examples

Force a specific status code:

```sh
curl -H 'Prefer: code=404' http://localhost:4010/pets/1
curl 'http://localhost:4010/pets/1?__code=404'
```

Pick a named example:

```sh
curl -H 'Prefer: example=fido' http://localhost:4010/pets/1
```

Switch to dynamic generation for a single request, with a deterministic
seed:

```sh
curl -H 'Prefer: dynamic=true, seed=42' http://localhost:4010/pets
```

Force the static example pipeline even when the server was started with
`--dynamic`:

```sh
chasm-server etc/petstore.yaml --dynamic &
curl -H 'Prefer: dynamic=false' http://localhost:4010/pets
```

Combine directives:

```sh
curl -H 'Prefer: code=201, example=newPet' http://localhost:4010/pets
```

## Response selection pipeline

When `code=` is set, the engine resolves the exact status code first, then
range keys (`2XX`, `4XX`, etc.), then the `default` response. With no `code=`
directive, the engine picks the lowest numeric `2xx` response, falling back
to `default`, then to the first response in the map, then to a synthetic
`200`.

The body pipeline (after `code=` and `dynamic=` have been applied):

1. Named example via `example=`/`__example` from the `examples` map.
2. First entry in the `examples` map (with `#/components/examples/X`
   resolution).
3. Inline `example` field on the media type.
4. `schema.example` on the response's schema, if present.
5. Schema-driven generation via `chasm-faker`. Honours `seed=`.
6. `null` when no schema or media type is available.

Steps 1-4 are skipped when `dynamic=true` (per-request) or
`--ignore-examples` (server-wide).
