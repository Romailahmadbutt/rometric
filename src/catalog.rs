//! Profile categorisation.
//!
//! The shipped AutoEQ asset only contains a name and a correction curve, so the
//! two useful ways to organise the library are derived from that data:
//!
//! * **Brand** — the manufacturer prefix of the profile name. A curated list
//!   keeps multi-word brands such as `64 Audio` and `Bowers & Wilkins` whole.
//! * **Sound** — the dominant region and direction of the correction curve.
//!   Folders describe what the EQ *does* to the signal, not how the headphone
//!   is marketed, so the grouping cannot claim metadata the database lacks.

use serde::{Deserialize, Serialize};

/// How the profile browser arranges its results.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grouping {
    #[default]
    All,
    Brand,
    Sound,
    Favorites,
    Recent,
}

impl Grouping {
    pub const ALL: [Grouping; 5] = [
        Grouping::All,
        Grouping::Brand,
        Grouping::Sound,
        Grouping::Favorites,
        Grouping::Recent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Grouping::All => "All",
            Grouping::Brand => "Brand",
            Grouping::Sound => "Sound",
            Grouping::Favorites => "Favorites",
            Grouping::Recent => "Recent",
        }
    }

    pub fn is_grouped(self) -> bool {
        matches!(self, Grouping::Brand | Grouping::Sound)
    }
}

/// Brands whose name contains a space. Longest match wins at runtime.
const MULTIWORD_BRANDS: &[&str] = &[
    "64 Audio",
    "7th Acoustics",
    "AFUL Acoustics",
    "ASTRO Gaming",
    "Alpha Omega",
    "Ambient Acoustics",
    "Andover Audio",
    "Anker Soundcore",
    "Astell & Kern",
    "Audio Genetic",
    "Audio Zenith",
    "Aure Audio",
    "Auribus Acoustics",
    "Austrian Audio",
    "Bang & Olufsen",
    "Binary Acoustics",
    "Bowers & Wilkins",
    "Campfire Audio",
    "Cambridge Audio",
    "Cosmic Ears",
    "Craft Ears",
    "Custom Art",
    "Cypher Labs",
    "Dan Clark",
    "Dita Audio",
    "Effect Audio",
    "Empire Ears",
    "Fearless Audio",
    "Final Audio",
    "Fir Audio",
    "Fischer Audio",
    "Flare Audio",
    "FLC Technology",
    "Focus Audio",
    "Fosi Audio",
    "Gold Planar",
    "Grell Audio",
    "GS Audio",
    "HEDD Audio",
    "Heir Audio",
    "IMR Acoustics",
    "IO Audio",
    "ISN AUDIO",
    "Ice Lab",
    "Itsfit Lab",
    "Jade Audio",
    "JH Audio",
    "JLab Audio",
    "JM Audio",
    "JQ Audio",
    "Jomo Audio",
    "Kiwi Ears",
    "Kumitate Lab",
    "LH Labs",
    "LZ Hi-Fi",
    "Life Acoustics",
    "Lime Ears",
    "MEE Audio",
    "Mayflower Electronics",
    "Meyer Audio",
    "MiM Audio",
    "Modhouse Audio",
    "Moe Audio",
    "Muse HiFi",
    "NF ACOUS",
    "NS Audio",
    "NocturnaL Audio",
    "Noble Audio",
    "Nostalgia Audio",
    "Ollo Audio",
    "Origami Audio",
    "PMG Audio",
    "Pai Audio",
    "Peacock Audio",
    "Periodic Audio",
    "Plunge Audio",
    "Polk Audio",
    "Prisma Audio",
    "Punch Audio",
    "Queen of Audio",
    "Rosson Audio",
    "SMS Audio",
    "Sash Tres",
    "Shortbus Audio",
    "Signature Acoustics",
    "Simgot Audio",
    "Sivga Audio",
    "Sound Rhyme",
    "Status Audio",
    "Symphonium Audio",
    "Tansio Mirai",
    "Tin HiFi",
    "Tralucent Audio",
    "Turtle Beach",
    "Ultimate Ears",
    "Unique Melody",
    "Unknown Custom",
    "Venture Electronics",
    "Vision Ears",
    "Warwick Acoustics",
    "Wicked Audio",
    "Wiss Audio",
    "Zero Audio",
    "écoute Audio",
];

/// Manufacturer for a profile name.
///
/// Returns a slice of `name`, so no allocation is needed while grouping the
/// full library.
pub fn brand_of(name: &str) -> &str {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "Unknown";
    }
    let mut best: Option<&'static str> = None;
    for brand in MULTIWORD_BRANDS {
        if let Some(rest) = trimmed.strip_prefix(brand) {
            let is_prefix = rest.is_empty() || rest.starts_with(' ');
            let is_longer = best.is_none_or(|current| brand.len() > current.len());
            if is_prefix && is_longer {
                best = Some(brand);
            }
        }
    }
    match best {
        Some(brand) => &trimmed[..brand.len()],
        None => trimmed
            .split_whitespace()
            .next()
            .map_or("Unknown", |token| &trimmed[..token.len()]),
    }
}

/// Dominant character of a correction curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signature {
    AddsBass,
    CutsBass,
    AddsMids,
    CutsMids,
    AddsTreble,
    CutsTreble,
    Neutral,
}

impl Signature {
    pub const ORDER: [Signature; 7] = [
        Signature::AddsBass,
        Signature::CutsBass,
        Signature::AddsMids,
        Signature::CutsMids,
        Signature::AddsTreble,
        Signature::CutsTreble,
        Signature::Neutral,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Signature::AddsBass => "Adds bass",
            Signature::CutsBass => "Cuts bass",
            Signature::AddsMids => "Adds mids",
            Signature::CutsMids => "Cuts mids",
            Signature::AddsTreble => "Adds treble",
            Signature::CutsTreble => "Cuts treble",
            Signature::Neutral => "Neutral correction",
        }
    }
}

/// Classify a correction curve by its strongest bass, mid or treble move.
///
/// A positive mean means the correction raises that region, a negative mean
/// means it lowers it. Moves under 1 dB are treated as neutral so that tiny
/// measurement noise does not scatter profiles across folders.
pub fn signature_of(freqs: &[f32], gains: &[f32]) -> Signature {
    if freqs.is_empty() || freqs.len() != gains.len() {
        return Signature::Neutral;
    }
    let bass = mean_between(freqs, gains, 20.0, 200.0);
    let mids = mean_between(freqs, gains, 300.0, 2_000.0);
    let treble = mean_between(freqs, gains, 4_000.0, 10_000.0);

    let regions = [
        (bass, Signature::AddsBass, Signature::CutsBass),
        (mids, Signature::AddsMids, Signature::CutsMids),
        (treble, Signature::AddsTreble, Signature::CutsTreble),
    ];
    let (value, lift, cut) = regions
        .iter()
        .map(|(value, lift, cut)| (*value, *lift, *cut))
        .fold(
            (0.0f32, Signature::Neutral, Signature::Neutral),
            |best, item| {
                if item.0.abs() > best.0.abs() {
                    item
                } else {
                    best
                }
            },
        );

    if !value.is_finite() || value.abs() < 1.0 {
        Signature::Neutral
    } else if value > 0.0 {
        lift
    } else {
        cut
    }
}

fn mean_between(freqs: &[f32], gains: &[f32], low: f32, high: f32) -> f32 {
    let mut sum = 0.0;
    let mut count = 0usize;
    for (freq, gain) in freqs.iter().zip(gains.iter()) {
        if freq.is_finite() && gain.is_finite() && *freq >= low && *freq <= high {
            sum += gain;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        sum / count as f32
    }
}

/// A folder in the profile browser.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub label: String,
    pub profiles: Vec<usize>,
    /// True for the collated tail of brands that only have one profile.
    pub is_other: bool,
}

impl Group {
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

/// Precomputed categories for an AutoEQ database.
pub struct ProfileCatalog {
    pub names: Vec<String>,
    pub brands: Vec<String>,
    pub signatures: Vec<Signature>,
    pub brand_groups: Vec<Group>,
    pub signature_groups: Vec<Group>,
}

impl ProfileCatalog {
    pub fn build<T: AsRef<[f32]>>(names: &[String], freqs: &[f32], curves: &[T]) -> Self {
        let brands: Vec<String> = names
            .iter()
            .map(|name| brand_of(name).to_string())
            .collect();
        let signatures: Vec<Signature> = curves
            .iter()
            .map(|gains| signature_of(freqs, gains.as_ref()))
            .collect();

        let brand_groups = group_by_brands(names, &brands);
        let signature_groups = group_by_signature(&signatures);

        Self {
            names: names.to_vec(),
            brands,
            signatures,
            brand_groups,
            signature_groups,
        }
    }

    /// Groups for the chosen arrangement. `None` means the flat list is used.
    pub fn groups(&self, grouping: Grouping) -> Option<&[Group]> {
        match grouping {
            Grouping::Brand => Some(&self.brand_groups),
            Grouping::Sound => Some(&self.signature_groups),
            _ => None,
        }
    }
}

fn group_by_brands(names: &[String], brands: &[String]) -> Vec<Group> {
    let mut map: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, brand) in brands.iter().enumerate() {
        match map.iter_mut().find(|(label, _)| label == brand) {
            Some((_, profiles)) => profiles.push(index),
            None => map.push((brand.clone(), vec![index])),
        }
    }
    map.sort_by(|a, b| {
        a.0.to_lowercase()
            .cmp(&b.0.to_lowercase())
            .then(a.0.cmp(&b.0))
    });

    let mut groups = Vec::with_capacity(map.len() + 1);
    let mut others = Vec::new();
    for (label, mut profiles) in map {
        if profiles.len() < 2 {
            others.extend(profiles);
        } else {
            profiles.sort_by(|a, b| names[*a].to_lowercase().cmp(&names[*b].to_lowercase()));
            groups.push(Group {
                label,
                profiles,
                is_other: false,
            });
        }
    }
    if !others.is_empty() {
        others.sort_by(|a, b| names[*a].to_lowercase().cmp(&names[*b].to_lowercase()));
        groups.push(Group {
            label: "Other brands".into(),
            profiles: others,
            is_other: true,
        });
    }
    groups
}

fn group_by_signature(signatures: &[Signature]) -> Vec<Group> {
    Signature::ORDER
        .iter()
        .filter_map(|wanted| {
            let profiles: Vec<usize> = signatures
                .iter()
                .enumerate()
                .filter(|(_, signature)| *signature == wanted)
                .map(|(index, _)| index)
                .collect();
            (!profiles.is_empty()).then(|| Group {
                label: wanted.label().to_string(),
                profiles,
                is_other: false,
            })
        })
        .collect()
}

/// Case-insensitive search inside groups. Empty groups are removed.
pub fn filter_groups(groups: &[Group], names: &[String], query: &str) -> Vec<Group> {
    let query = query.trim().to_lowercase();
    groups
        .iter()
        .filter_map(|group| {
            if query.is_empty() {
                return Some(group.clone());
            }
            let profiles: Vec<usize> = group
                .profiles
                .iter()
                .copied()
                .filter(|index| {
                    names
                        .get(*index)
                        .is_some_and(|name| name.to_lowercase().contains(&query))
                })
                .collect();
            (!profiles.is_empty()).then(|| Group {
                label: group.label.clone(),
                profiles,
                is_other: group.is_other,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_curated_brand_matches_a_real_prefix_shape() {
        for brand in MULTIWORD_BRANDS {
            assert!(brand.contains(' ') || brand.contains('&'));
            assert_eq!(brand_of(&format!("{brand} Model")), *brand);
        }
    }

    #[test]
    fn empty_and_odd_names_do_not_panic() {
        assert_eq!(brand_of("   "), "Unknown");
        assert_eq!(brand_of("1234"), "1234");
        assert_eq!(signature_of(&[], &[]), Signature::Neutral);
        assert_eq!(signature_of(&[100.0], &[f32::NAN]), Signature::Neutral);
    }
}
