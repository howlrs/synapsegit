# Synapse Core OID Profile v0.1

Status: Stage 0 normative draft<br>
Protocol index: [SynapseGit Core Protocol v0.1](./README.md)

## 1. Goals

The same logical Core object must produce the same OID across supported
languages and runtimes. OID calculation must not depend on database IDs,
transport wrappers, access control, or mutable availability state.

## 2. Raw blobs

For a raw blob, calculate SHA-256 over the original byte sequence without
normalization or metadata injection.

```text
blob:sg-oid-v1:sha256:<64 lowercase hexadecimal characters>
```

The media type, filename, access policy, and encryption metadata are stored in
records that reference the blob. They are not included in the blob digest.

## 3. Structured objects

Records, manifest trees, and commits use Synapse Canonical JSON v0.1.

```text
record:sg-oid-v1:sha256:<domain-separated SHA-256>
tree:sg-oid-v1:sha256:<domain-separated SHA-256>
commit:sg-oid-v1:sha256:<domain-separated SHA-256>
```

Every structured object contains an `object_type` field matching the expected
OID prefix. Implementations must reject a prefix/body mismatch.

The digest preimage is:

```text
UTF8("synapsegit/core/v0.1\0" + object_type + "\0" + byte_length + "\0")
|| canonical_json_bytes
```

`byte_length` is the base-10 length of `canonical_json_bytes` without leading
zeroes. This domain-separates object classes and binds the canonical profile
version to the digest.

## 4. Synapse Canonical JSON v0.1

Before hashing, an implementation must:

1. decode structured input as UTF-8 in fatal mode; reject invalid UTF-8 and an
   input byte-order mark;
2. parse JSON while retaining lexical information and rejecting duplicate
   object keys after JSON escape decoding;
3. accept a number token only when it matches `-?(0|[1-9][0-9]*)`; reject
   fractions, exponent notation, `-0`, and integers outside the JavaScript safe
   integer range before conversion to a runtime number;
4. reject lone UTF-16 surrogate code points in object keys and string values;
5. require every object key to already be Unicode NFC and reject it otherwise;
6. require string values marked `x-synapse-string: identifier-nfc` by the
   applied schema to already be NFC; identifiers with an ASCII-only grammar
   satisfy this automatically;
7. preserve all other string values exactly as parsed; creative text, prompts,
   labels, and quotations are never Unicode-normalized by the canonicalizer;
8. sort object keys in ascending UTF-16 code-unit order, including
   integer-looking keys such as `"10"` and `"2"`;
9. serialize strings with the JSON string serialization defined by
   [RFC 8785 section 3.2.2.2](https://www.rfc-editor.org/rfc/rfc8785#section-3.2.2.2),
   while using this profile's restricted integer rule;
10. preserve array order and serialize without insignificant whitespace;
11. encode the result as UTF-8 without a byte-order mark or trailing newline.

These checks occur before a generic `JSON.parse`-style API can discard duplicate
keys or number-token spelling. Implementations must not rebuild a sorted object
and then rely on a runtime's property enumeration order; canonical object
members are emitted directly in the sorted order.

Schemas may mark arrays with `x-synapse-order: set` or
`x-synapse-order: sequence`. Sequence arrays preserve semantic order. Set arrays
must already be sorted by the canonical bytes of each item and contain no
duplicates; the canonicalizer validates but does not silently reorder them.
An unannotated array, including one inside an unregistered extension, is a
sequence. Every array in a Core v0.1 schema is explicitly annotated. A set
annotation supplied by an extension is enforceable only when that extension's
schema is available to the verifier.

Measurements requiring fractions use a normalized structured fixed-point value:

```json
{
  "mantissa": "125",
  "scale": -2,
  "unit": "mm"
}
```

This represents `1.25 mm`. Non-zero mantissas have no trailing zero. Zero is
always `{ "mantissa": "0", "scale": 0, ... }`. Units come from the Core v0.1
unit vocabulary. The string mantissa keeps values larger than a runtime's safe
integer range lossless.

## 5. Time

Hashed timestamps use exactly `YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ`. Offset timestamps and
other fractional precision are rejected at the content-addressed boundary; a
producer must construct a separate canonical value before `put`. Calendar
validity is checked semantically in addition to the schema regex. A time
interval or an unknown time is represented as a typed object; the actual clock
precision is represented separately and must not be inferred from zeroes. Absent precision
must not be invented.

## 6. Paths

Manifest path segments are object keys and must already be NFC. Other identifier
strings are either ASCII by grammar or explicitly marked
`x-synapse-string: identifier-nfc`. A ManifestTree node contains only single
path segments; hierarchy is represented by child tree OIDs. Segments:

- are relative;
- contain no `/` separator;
- are not empty, `.` or `..`;
- contain no NUL;
- are case-sensitive in v0.1.

## 7. OID verification

Consumers first validate the concrete object schema, then recalculate the
domain-separated digest from the received body. Records are dispatched through
`schemas/record.schema.json`; validating only the common Envelope is
insufficient. An API-supplied OID that does not match, or whose prefix conflicts
with `object_type`, is rejected. A valid digest proves byte identity only; it
does not prove authorship, capture time, truth, copyright, or permission.

## 8. Non-goals

This profile does not canonicalize arbitrary pre-existing JSON. It defines the
restricted JSON domain accepted for Synapse Core content-addressed objects.
