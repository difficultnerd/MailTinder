//! The three v1 personas (T-1120a shape): interests, name pools and the size
//! of a persona's sender roster.
//!
//! Every name here is invented. No real brand, logo or person appears in this
//! crate (rule 2), and every address the caller builds from these pools uses a
//! reserved domain.

/// One persona.
pub struct Persona {
    pub name: &'static str,
    /// Subjects and prose mention these; keeps bodies persona-flavoured.
    pub interests: &'static [&'static str],
    /// Personal correspondents' first names (family, friends, classmates).
    pub people: &'static [&'static str],
    /// Sender count range, inclusive: 40 to 120 recurring senders.
    pub senders: (usize, usize),
    /// Correspondent count range, inclusive: 5 to 15.
    pub correspondents: (usize, usize),
}

/// The personas v1 knows, in a stable order.
pub const NAMES: [&str; 3] = ["student", "freelancer", "parent"];

/// Look a persona up by name; an unknown name is an argument error.
pub fn by_name(name: &str) -> Result<Persona, String> {
    match name {
        "student" => Ok(STUDENT),
        "freelancer" => Ok(FREELANCER),
        "parent" => Ok(PARENT),
        other => Err(format!(
            "unknown persona `{other}`: expected one of {}",
            NAMES.join(", ")
        )),
    }
}

const STUDENT: Persona = Persona {
    name: "student",
    interests: &[
        "lecture notes",
        "society socials",
        "part-time shifts",
        "library loans",
        "cheap travel",
        "second-hand books",
    ],
    people: &[
        "Ada", "Bo", "Cleo", "Dev", "Elif", "Finn", "Greta", "Hana", "Ines", "Jonas", "Kira",
        "Luiz", "Mila", "Noor", "Otto",
    ],
    senders: (40, 120),
    correspondents: (5, 15),
};

const FREELANCER: Persona = Persona {
    name: "freelancer",
    interests: &[
        "client invoices",
        "design tools",
        "conference tickets",
        "tax paperwork",
        "coworking passes",
        "podcast hosting",
    ],
    people: &[
        "Anya", "Bruno", "Cara", "Dmitri", "Esme", "Farid", "Gita", "Hugo", "Ida", "Jules",
        "Karel", "Lena", "Mara", "Nils", "Orla",
    ],
    senders: (40, 120),
    correspondents: (5, 15),
};

const PARENT: Persona = Persona {
    name: "parent",
    interests: &[
        "school newsletters",
        "football fixtures",
        "grocery orders",
        "vet reminders",
        "holiday cottages",
        "birthday parties",
    ],
    people: &[
        "Bea", "Callum", "Dora", "Emil", "Freya", "Gus", "Hilda", "Ivo", "Jana", "Kit", "Lars",
        "Mona", "Nadia", "Piet", "Rosa",
    ],
    senders: (40, 120),
    correspondents: (5, 15),
};
