# Creator decision pins v1

This Creator-only immutable contract lives in the DecisionFeedback envelope at
`extensions["org.synapsegit.creator-decision-pins"]`. It adds no payload field,
SpatialFrame or `scope_regions` semantics. `format` is exactly
`synapsegit-creator-decision-pins-v1`; `pins` is a sequence of at most 10 objects.
Sequence order is displayed as 1-based numbering, not a durable separate ID.
Each pin has exactly `role`, `blob_oid`, `x`, `y`, `note`. Unknown fields are invalid.

- `role` is `original`, `current` or `ai_output` in the exact reviewed Proposal.
- `blob_oid` must equal the verified Blob OID of that role. It is an equality
  binding, never an arbitrary lookup key or publication authority.
- `x` and `y` are integers in [0, 1000000]. (0,0) is the top-left outer corner;
  (1000000,1000000) is the bottom-right outer corner of the whole oriented decoded
  raster canvas, including transparent padding. Stored coordinates are fractions
  of the full width/height, not window coordinates, pixel indexes or floats.
- Browser decoding applies embedded orientation (`image-orientation: from-image`).
  The pin canvas uses that whole oriented image, without cropping, registration
  or an additional rotation. Its natural decoded dimensions determine 100%/200%
  sizing; viewport size and zoom never change stored coordinates. Animated images
  bind their full image canvas, not an animation frame or timestamp. The contract
  asserts no equivalence between independently different decoder implementations.
- `note` is nonempty plain text, at most 200 UTF-8 bytes; no truncation.

The extension is written only with the ordinary one-shot Human DecisionFeedback,
private visibility and training prohibited. Rationale and disposition are in that
same Record, which binds the exact Proposal Commit. Pins do not imply analysis,
physical regions, byte identity, partial adoption or adoption of an individual pin.
No pin is published separately. Failed validation does not consume pending
review authority or advance the Decision Ref. CAS orphans after a later failure
are not published decisions. Public bundles do not copy these notes.

The localhost actual JSON request (review ID, disposition, rationale, annotations,
JSON escaping and whitespace) must fit 8192 bytes. The HTTP transport checks the
raw body; the service additionally bounds its compact serialized request. Existing
5000-byte rationale and pin limits are checked independently. The UI displays the
remaining bytes for each disposition and disables oversize submission.

Reports validate the decision lineage before decoding this extension. Absent
extension means no recorded pins. Unknown version/fields, invalid coordinates or
mismatched image binding produce `annotations_unavailable: true` and no pins;
the separately verified decision remains readable. Attachment/decode failure
prevents interactive pin display/creation, with a visible explanation. The service
validates exact opaque Blob binding, not visual content or decoder behavior.

Normal private Core archives retain the feedback and referenced proposal images,
so restore keeps positions, order and text. No post-decision editing, partial
adoption, automatic image matching, persistent drafts or restart recovery exists.
`valid.json` is an extension fixture; Rust validates it independently of rendering.
