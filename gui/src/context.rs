use std::fmt;

use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
    ser::SerializeStruct,
};
use slint::{ModelRc, SharedString};

use crate::DvdbndCheck;

#[derive(Default, Serialize, Deserialize)]
pub struct AppContext {
    pub mount_path: SharedString,
    pub keys_path: SharedString,
    pub dict_path: SharedString,
    pub cache_path: SharedString,
    pub use_cache: bool,
    #[serde(with = "GameDirs")]
    pub game_dirs: crate::GameDirs,
    pub dvdbnds: ModelRc<DvdbndCheck>,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::GameDirs")]
pub struct GameDirs {
    pub dirs: ModelRc<SharedString>,
    pub active_index: i32,
}

impl Serialize for DvdbndCheck {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("DvdbndCheck", 2)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("checked", &self.checked)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for DvdbndCheck {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Name,
            Checked,
        }

        struct DvdbndCheckVisitor;

        impl<'de> Visitor<'de> for DvdbndCheckVisitor {
            type Value = DvdbndCheck;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct Duration")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<Self::Value, V::Error>
            where
                V: SeqAccess<'de>,
            {
                let name = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let checked = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                Ok(DvdbndCheck { name, checked })
            }

            fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut name = None;
                let mut checked = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Name => {
                            if name.is_some() {
                                return Err(de::Error::duplicate_field("name"));
                            }
                            name = Some(map.next_value()?);
                        }
                        Field::Checked => {
                            if checked.is_some() {
                                return Err(de::Error::duplicate_field("checked"));
                            }
                            checked = Some(map.next_value()?);
                        }
                    }
                }
                let name = name.ok_or_else(|| de::Error::missing_field("name"))?;
                let checked = checked.ok_or_else(|| de::Error::missing_field("checked"))?;
                Ok(DvdbndCheck { name, checked })
            }
        }

        const FIELDS: &[&str] = &["name", "checked"];
        deserializer.deserialize_struct("Duration", FIELDS, DvdbndCheckVisitor)
    }
}
