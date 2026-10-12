//! Sender archetypes, the persona's recurring-sender roster and the send
//! schedule each sender follows (T-1120a shape).
//!
//! Every address is built from a reserved domain: `example.com`, `example.net`,
//! `example.org`, `*.test` or `*.invalid` (rule 2). Display names, subject
//! pools and list IDs are invented; nothing here names a real brand.

use std::collections::BTreeSet;

use domain::MessageClass;

use crate::synth::persona::Persona;
use crate::synth::rng::SplitMix64;

/// Messages span 1 to 10 years before `--epoch`.
pub const MAX_AGE_DAYS: i64 = 3650;

/// The kinds of recurring sender a persona is exposed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Archetype {
    Retailer,
    Receipt,
    BankNotice,
    Newsletter,
    SaasNotice,
    Community,
    Travel,
    Utility,
    Social,
    Family,
    Campus,
    Clinic,
    School,
    Club,
    Spam,
}

/// Every archetype, in the order the weighting walks them (stable).
pub const ARCHETYPES: [Archetype; 15] = [
    Archetype::Retailer,
    Archetype::Receipt,
    Archetype::BankNotice,
    Archetype::Newsletter,
    Archetype::SaasNotice,
    Archetype::Community,
    Archetype::Travel,
    Archetype::Utility,
    Archetype::Social,
    Archetype::Family,
    Archetype::Campus,
    Archetype::Clinic,
    Archetype::School,
    Archetype::Club,
    Archetype::Spam,
];

impl Archetype {
    /// The class the S3 names give this kind of sender.
    pub fn class(self) -> MessageClass {
        match self {
            Self::Retailer
            | Self::Newsletter
            | Self::Community
            | Self::Travel
            | Self::Campus
            | Self::Club => MessageClass::List,
            Self::Receipt | Self::BankNotice | Self::Utility | Self::Clinic | Self::School => {
                MessageClass::Notice
            }
            Self::SaasNotice | Self::Social => MessageClass::BulkNoHeader,
            Self::Family => MessageClass::Personal,
            Self::Spam => MessageClass::Suspect,
        }
    }

    /// Relative frequency in a roster (sums to 105).
    pub const fn weight(self) -> u32 {
        match self {
            Self::Retailer | Self::Receipt | Self::SaasNotice | Self::Spam => 8,
            Self::BankNotice | Self::Travel => 5,
            Self::Newsletter => 12,
            Self::Community | Self::Social => 7,
            Self::Utility => 6,
            Self::Family => 14,
            Self::Campus | Self::Clinic | Self::School | Self::Club => 4,
        }
    }

    pub fn displays(self) -> &'static [&'static str] {
        match self {
            Self::Retailer => &["Marigold Supplies", "Tarn & Co", "Bluebell Home"],
            Self::Receipt => &["Marigold Supplies Orders", "Tarn & Co Receipts"],
            Self::BankNotice => &["Fenwick Bank", "Harbour Savings"],
            Self::Newsletter => &["The Weekly Ledger", "Northwind Digest", "Field Notes"],
            Self::SaasNotice => &["Plotter Cloud", "Timeline App", "Audiotool Studio"],
            Self::Community => &["Riverside Neighbours", "Lantern Society"],
            Self::Travel => &["Coastline Rail", "Harbour Ferries", "Wander Cottages"],
            Self::Utility => &["Fenwick Water", "Grid Electric", "City Waste"],
            Self::Social => &["Chatterbox", "Pingline"],
            Self::Family => &["Family", "Home"],
            Self::Campus => &["Ashford University", "Ashford Library"],
            Self::Clinic => &["Bridge Street Clinic", "Northgate Dental"],
            Self::School => &["Elmwood Primary", "Hilltop School"],
            Self::Club => &["Harbour Runners", "Studio Pottery Club"],
            Self::Spam => &[
                "Fenwick Bank Security",
                "Account Team",
                "Delivery Notice",
                "Prize Desk",
            ],
        }
    }

    pub fn locals(self) -> &'static [&'static str] {
        match self {
            Self::Retailer => &["orders", "shop", "hello"],
            Self::Receipt => &["receipts", "orders"],
            Self::BankNotice => &["notices", "statements"],
            Self::Newsletter => &["news", "digest", "weekly"],
            Self::SaasNotice => &["notifications", "no-reply", "alerts"],
            Self::Community => &["neighbours", "committee"],
            Self::Travel => &["bookings", "rail", "stays"],
            Self::Utility => &["billing", "metering"],
            Self::Social => &["notifications", "hello"],
            Self::Family => &["mum", "dad", "home"],
            Self::Campus => &["library", "registry"],
            Self::Clinic => &["appointments", "reception"],
            Self::School => &["office", "newsletter"],
            Self::Club => &["members", "coach"],
            Self::Spam => &["security", "verify", "no-reply", "delivery"],
        }
    }

    pub fn domains(self) -> &'static [&'static str] {
        match self {
            Self::Retailer | Self::Receipt => &["shop.example.com", "orders.example.net"],
            Self::BankNotice => &["notices.example.com", "statements.example.net"],
            Self::Newsletter => &[
                "news.example.org",
                "digest.example.com",
                "weekly.example.net",
            ],
            Self::SaasNotice => &["notify.example.com", "app.example.net"],
            Self::Community => &["community.example.org", "neighbours.test"],
            Self::Travel => &["travel.example.net", "stays.example.org"],
            Self::Utility => &["utility.example.org", "billing.example.com"],
            Self::Social => &["social.example.net", "ping.example.com"],
            Self::Family => &["family.test", "home.example.org"],
            Self::Campus => &["campus.example.org", "library.example.net"],
            Self::Clinic => &["clinic.example.org", "dental.example.net"],
            Self::School => &["school.example.net", "primary.example.org"],
            Self::Club => &["club.example.org", "runners.example.net"],
            Self::Spam => &["secure-notice.example.com", "account-check.test"],
        }
    }

    pub fn subjects(self) -> &'static [&'static str] {
        match self {
            Self::Retailer => &["Autumn sale this week", "New arrivals in store"],
            Self::Receipt => &[
                "Your order receipt",
                "Order confirmed",
                "Your parcel is on its way",
            ],
            Self::BankNotice => &["Your statement is ready", "A notice about your account"],
            Self::Newsletter => &["This week in review", "Your weekly digest"],
            Self::SaasNotice => &["Your workspace summary", "Two mentions waiting"],
            Self::Community => &["Street clean-up on Saturday", "Minutes from the meeting"],
            Self::Travel => &["Your booking details", "Platform change notice"],
            Self::Utility => &["Your bill is ready", "Meter reading reminder"],
            Self::Social => &["You have new reactions", "Someone tagged you"],
            Self::Family => &["Weekend plans", "Photos from Sunday"],
            Self::Campus => &["Library loan reminder", "Module enrolment opens"],
            Self::Clinic => &["Appointment reminder", "Test results ready"],
            Self::School => &["Term dates for next year", "School newsletter"],
            Self::Club => &["Sunday run is on", "Studio booking open"],
            Self::Spam => &[
                "Urgent: verify your account",
                "Your parcel could not be delivered",
                "Final notice: payment required",
            ],
        }
    }

    /// Senders whose calendar has sale-shaped bursts.
    fn has_bursts(self) -> bool {
        matches!(
            self,
            Self::Retailer | Self::Newsletter | Self::Travel | Self::Club | Self::Social
        )
    }
}

/// How often a sender writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    Daily,
    Weekly,
    Monthly,
}

impl Cadence {
    /// Days between sends.
    pub const fn step_days(self) -> i64 {
        match self {
            Self::Daily => 1,
            Self::Weekly => 7,
            Self::Monthly => 30,
        }
    }
}

/// A mid-lifetime address change.
#[derive(Clone, Debug)]
pub struct AddressChange {
    /// Days before epoch; messages newer than this use the new address.
    pub day: i64,
    pub address: String,
}

/// One recurring sender.
#[derive(Clone, Debug)]
pub struct Sender {
    pub index: usize,
    pub archetype: Archetype,
    pub class: MessageClass,
    pub display: String,
    pub address: String,
    pub domain: String,
    pub cadence: Cadence,
    pub step_days: i64,
    /// Days before epoch: the sender's first message.
    pub first_day: i64,
    /// Days before epoch: the sender's most recent message (0 = still active).
    pub last_day: i64,
    pub change: Option<AddressChange>,
    /// Days before epoch at the start of each sale-shaped burst.
    pub bursts: Vec<i64>,
    pub list_id: String,
    pub interest: &'static str,
    pub personal: bool,
}

impl Sender {
    /// The address this sender writes from on a given day.
    pub fn address_on(&self, day: i64) -> &str {
        match &self.change {
            Some(c) if day < c.day => &c.address,
            _ => &self.address,
        }
    }

    /// The subject pool for this sender's archetype.
    pub fn subjects(&self) -> &'static [&'static str] {
        self.archetype.subjects()
    }
}

/// Build the roster: `senders` recurring senders drawn from the archetypes,
/// then `correspondents` personal ones (the family and friends of the persona).
pub fn roster(
    rng: &mut SplitMix64,
    seed: u64,
    persona: &Persona,
    senders: usize,
    correspondents: usize,
) -> Vec<Sender> {
    let mut out: Vec<Sender> = Vec::with_capacity(senders + correspondents);
    let mut used: BTreeSet<String> = BTreeSet::new();
    for index in 0..senders {
        let archetype = weighted_pick(rng);
        out.push(build(rng, persona, index, archetype, &mut used));
    }
    for index in 0..correspondents {
        let person = rng.pick(persona.people);
        let local = person.to_lowercase();
        let address = unique_address(&format!("{local}@family.test"), index, &mut used);
        let domain = "family.test".to_owned();
        // A correspondent's own schedule: its own stream, off the same seed.
        let mut r = build(
            &mut SplitMix64::stream(seed, &address),
            persona,
            senders + index,
            Archetype::Family,
            &mut used,
        );
        (*person).clone_into(&mut r.display);
        r.address = address;
        r.personal = true;
        "people.family.test".clone_into(&mut r.list_id);
        r.domain = domain;
        out.push(r);
    }
    out
}

/// A stable weighted draw over [`ARCHETYPES`].
fn weighted_pick(rng: &mut SplitMix64) -> Archetype {
    let total: u64 = ARCHETYPES.iter().map(|a| u64::from(a.weight())).sum();
    let mut pick = rng.below(total);
    let mut chosen = Archetype::Retailer;
    for a in ARCHETYPES {
        let w = u64::from(a.weight());
        if pick < w {
            chosen = a;
            break;
        }
        pick -= w;
    }
    chosen
}

/// Build one non-correspondent sender.
fn build(
    rng: &mut SplitMix64,
    persona: &Persona,
    index: usize,
    archetype: Archetype,
    used: &mut BTreeSet<String>,
) -> Sender {
    let display = (*rng.pick(archetype.displays())).to_owned();
    let local = *rng.pick(archetype.locals());
    let domain = (*rng.pick(archetype.domains())).to_owned();
    let address = unique_address(&format!("{local}@{domain}"), index, used);

    let first_day = skewed_day(rng);
    let mut last_day = if rng.chance(200) {
        days(rng, 60, (first_day - 45).max(61))
    } else {
        days(rng, 0, 21)
    };
    if last_day + 30 > first_day {
        last_day = (first_day - 31).max(0);
    }

    let cadence = match rng.below(1000) {
        0..=199 => Cadence::Daily,
        200..=699 => Cadence::Weekly,
        _ => Cadence::Monthly,
    };

    let mut bursts: Vec<i64> = Vec::new();
    if archetype.has_bursts() {
        for _ in 0..rng.range(0, 3) {
            bursts.push(days(rng, last_day, first_day));
        }
        bursts.sort_unstable();
        bursts.dedup();
    }

    let change = rng.chance(150).then(|| {
        let day = days(rng, last_day + 5, (first_day - 5).max(last_day + 6));
        let alt_local = *rng.pick(archetype.locals());
        let alt_domain = (*rng.pick(archetype.domains())).to_owned();
        AddressChange {
            day,
            address: unique_address(&format!("{alt_local}@{alt_domain}"), index + 900, used),
        }
    });

    let list_id = format!("{local}.{domain}");
    Sender {
        index,
        archetype,
        class: archetype.class(),
        display,
        address,
        domain,
        cadence,
        step_days: cadence.step_days(),
        first_day,
        last_day,
        change,
        bursts,
        list_id,
        interest: rng.pick(persona.interests),
        personal: false,
    }
}

/// A day range, in the `i64` the schedule uses.
fn days(rng: &mut SplitMix64, lo: i64, hi: i64) -> i64 {
    let lo = u64::try_from(lo).unwrap_or(0);
    let hi = u64::try_from(hi).unwrap_or(0);
    i64::try_from(rng.range(lo, hi)).unwrap_or(0)
}

/// A day 1..=[`MAX_AGE_DAYS`] before epoch, skewed to recent.
pub fn skewed_day(rng: &mut SplitMix64) -> i64 {
    let x = rng.below(u64::try_from(MAX_AGE_DAYS).unwrap_or(3650));
    let day = (x * x) / u64::try_from(MAX_AGE_DAYS).unwrap_or(3650);
    i64::try_from(day).unwrap_or(0) + 1
}

/// Keep addresses unique inside one roster.
fn unique_address(base: &str, salt: usize, used: &mut BTreeSet<String>) -> String {
    let candidate = base.to_owned();
    if used.insert(candidate.clone()) {
        return candidate;
    }
    if let Some((local, domain)) = base.split_once('@') {
        let candidate = format!("{local}.{salt}@{domain}");
        used.insert(candidate.clone());
        return candidate;
    }
    candidate
}
