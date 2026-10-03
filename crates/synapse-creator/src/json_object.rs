//! Object-only serde for the JSON contracts that this crate reads as input.
//!
//! serde's derived struct deserialization also accepts a positional JSON
//! array, so `["tool", "model", "prompt", "intent"]` would silently become a
//! generation note.  The generation note and the import inbox manifest are
//! JSON objects by contract, so their types derive serde with
//! `#[serde(remote = "Self")]`, which turns the derived code into inherent
//! functions, and use `json_object_serde!` for the trait implementations:
//! serialization is unchanged and deserialization accepts only a JSON object.

/// Implement `Serialize` and an object-only `Deserialize` for a struct whose
/// serde derive uses `#[serde(remote = "Self")]`.
macro_rules! json_object_serde {
    ($type:ident) => {
        impl serde::Serialize for $type {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                $type::serialize(self, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $type {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                struct ObjectVisitor;

                impl<'de> serde::de::Visitor<'de> for ObjectVisitor {
                    type Value = $type;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("a JSON object")
                    }

                    fn visit_map<A: serde::de::MapAccess<'de>>(
                        self,
                        map: A,
                    ) -> std::result::Result<$type, A::Error> {
                        $type::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    }
                }

                deserializer.deserialize_map(ObjectVisitor)
            }
        }
    };
}
