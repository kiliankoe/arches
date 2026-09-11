//! Integer enums from LocoKit2, kept faithful to the upstream raw values.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A name that does not match any known case and is not a bare raw value either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownName {
    pub enum_name: &'static str,
    pub value: String,
}

impl fmt::Display for UnknownName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a {} name", self.value, self.enum_name)
    }
}

impl std::error::Error for UnknownName {}

/// Arc ships new enum cases before we hear about them, so every enum carries an `Other` escape
/// hatch: one unrecognised raw value must never fail a whole bucket file. Serialization is by
/// name because the API speaks names, while Arc's JSON only ever holds the integer.
macro_rules! raw_int_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($variant:ident = $raw:literal => $label:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)+
            Other(i32),
        }

        impl $name {
            /// Every case upstream declares, in raw value order. Handy for exhaustive tests.
            #[allow(dead_code, reason = "phase 4 renders enum names in the API")]
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn from_raw(raw: i32) -> Self {
                match raw {
                    $($raw => $name::$variant,)+
                    other => $name::Other(other),
                }
            }

            pub fn raw(self) -> i32 {
                match self {
                    $($name::$variant => $raw,)+
                    $name::Other(raw) => raw,
                }
            }

            /// The upstream camelCase case name; unknown cases render as their raw value.
            pub fn as_str(self) -> Cow<'static, str> {
                match self {
                    $($name::$variant => Cow::Borrowed($label),)+
                    $name::Other(raw) => Cow::Owned(raw.to_string()),
                }
            }
        }

        impl FromStr for $name {
            type Err = UnknownName;

            fn from_str(name: &str) -> Result<Self, Self::Err> {
                match name {
                    $($label => Ok($name::$variant),)+
                    // Mirror `as_str`, so a name that went out over the API comes back in.
                    other => other.parse::<i32>().map($name::Other).map_err(|_| UnknownName {
                        enum_name: stringify!($name),
                        value: other.to_string(),
                    }),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Ok(Self::from_raw(i32::deserialize(deserializer)?))
            }
        }
    };
}

raw_int_enum! {
    /// `LocomotionSample.classifiedActivityType`, `confirmedActivityType` and the trip equivalents.
    ActivityType {
        Unknown = -1 => "unknown",
        Bogus = 0 => "bogus",
        Stationary = 1 => "stationary",
        Walking = 2 => "walking",
        Running = 3 => "running",
        Cycling = 4 => "cycling",
        Car = 5 => "car",
        Airplane = 6 => "airplane",
        Train = 20 => "train",
        Bus = 21 => "bus",
        Motorcycle = 22 => "motorcycle",
        Boat = 23 => "boat",
        Tram = 24 => "tram",
        Tractor = 25 => "tractor",
        Tuktuk = 26 => "tuktuk",
        Songthaew = 27 => "songthaew",
        Scooter = 28 => "scooter",
        Metro = 29 => "metro",
        CableCar = 30 => "cableCar",
        Funicular = 31 => "funicular",
        Chairlift = 32 => "chairlift",
        SkiLift = 33 => "skiLift",
        Taxi = 34 => "taxi",
        HotAirBalloon = 35 => "hotAirBalloon",
        Skateboarding = 50 => "skateboarding",
        InlineSkating = 51 => "inlineSkating",
        Snowboarding = 52 => "snowboarding",
        Skiing = 53 => "skiing",
        Horseback = 54 => "horseback",
        Swimming = 55 => "swimming",
        Golf = 56 => "golf",
        Wheelchair = 57 => "wheelchair",
        Rowing = 58 => "rowing",
        Kayaking = 59 => "kayaking",
        Surfing = 60 => "surfing",
        Hiking = 61 => "hiking",
    }
}

impl ActivityType {
    /// Mirrors upstream `isMovingType`: an unknown future case is assumed to be movement, since
    /// the only non-moving cases are the three that mean "not going anywhere".
    #[allow(dead_code, reason = "phase 3 splits distance by moving type")]
    pub fn is_moving_type(self) -> bool {
        !matches!(
            self,
            ActivityType::Unknown | ActivityType::Bogus | ActivityType::Stationary
        )
    }
}

raw_int_enum! {
    /// `LocomotionSample.movingState`.
    MovingState {
        Uncertain = -1 => "uncertain",
        Stationary = 0 => "stationary",
        Moving = 1 => "moving",
    }
}

raw_int_enum! {
    /// `LocomotionSample.recordingState`.
    RecordingState {
        Off = 0 => "off",
        Recording = 1 => "recording",
        Sleeping = 2 => "sleeping",
        DeepSleeping = 3 => "deepSleeping",
        Wakeup = 4 => "wakeup",
        Standby = 5 => "standby",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(values: &[T])
    where
        T: Copy + fmt::Debug + PartialEq + FromStr + Serialize,
        T::Err: fmt::Debug,
    {
        for &value in values {
            let name = serde_json::to_value(value).unwrap();
            let name = name.as_str().unwrap();
            assert_eq!(T::from_str(name).unwrap(), value, "name round trip {name}");
        }
    }

    #[test]
    fn activity_types_round_trip_through_raw_and_name() {
        for &value in ActivityType::ALL {
            assert_eq!(ActivityType::from_raw(value.raw()), value);
        }
        round_trip(ActivityType::ALL);
        assert_eq!(ActivityType::from_raw(24), ActivityType::Tram);
        assert_eq!(ActivityType::Tram.as_str(), "tram");
        assert_eq!(ActivityType::CableCar.as_str(), "cableCar");
        assert_eq!(ActivityType::Unknown.as_str(), "unknown");
        assert_eq!(ActivityType::Bogus.as_str(), "bogus");
    }

    #[test]
    fn moving_and_recording_states_round_trip() {
        for &value in MovingState::ALL {
            assert_eq!(MovingState::from_raw(value.raw()), value);
        }
        for &value in RecordingState::ALL {
            assert_eq!(RecordingState::from_raw(value.raw()), value);
        }
        round_trip(MovingState::ALL);
        round_trip(RecordingState::ALL);
    }

    #[test]
    fn unknown_raw_values_become_other() {
        let parsed: ActivityType = serde_json::from_str("99").unwrap();
        assert_eq!(parsed, ActivityType::Other(99));
        assert_eq!(parsed.raw(), 99);
        assert_eq!(parsed.as_str(), "99");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "\"99\"");
        assert_eq!(ActivityType::from_str("99").unwrap(), parsed);

        assert_eq!(MovingState::from_raw(7), MovingState::Other(7));
        assert_eq!(RecordingState::from_raw(-3), RecordingState::Other(-3));
    }

    #[test]
    fn unparseable_names_are_rejected() {
        let error = ActivityType::from_str("teleporting").unwrap_err();
        assert_eq!(error.enum_name, "ActivityType");
        assert!(MovingState::from_str("hovering").is_err());
    }

    #[test]
    fn moving_classification_matches_upstream() {
        for &value in ActivityType::ALL {
            let expected = !matches!(
                value,
                ActivityType::Unknown | ActivityType::Bogus | ActivityType::Stationary
            );
            assert_eq!(value.is_moving_type(), expected, "{value}");
        }
        assert!(!ActivityType::Stationary.is_moving_type());
        assert!(ActivityType::Tram.is_moving_type());
        // A future case is assumed to move; only the three known idle cases are not.
        assert!(ActivityType::Other(99).is_moving_type());
    }
}
