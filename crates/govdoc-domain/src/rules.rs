use crate::schemas::RecipientClass;

pub fn lookup_salutation(recipient_class: &RecipientClass) -> &'static str {
    match recipient_class {
        RecipientClass::GeneralPublic => "เรียน",
        RecipientClass::JuniorOfficial => "เรียน",
        RecipientClass::SeniorOfficial => "เรียน",
        RecipientClass::Executive => "กราบเรียน",
        RecipientClass::Monk => "กราบนมัสการ",
        RecipientClass::Royal => "กราบบังคมทูล",
    }
}

pub fn lookup_closing(recipient_class: &RecipientClass) -> &'static str {
    match recipient_class {
        RecipientClass::GeneralPublic => "ขอแสดงความนับถือ",
        RecipientClass::JuniorOfficial => "ขอแสดงความนับถือ",
        RecipientClass::SeniorOfficial => "ขอแสดงความนับถืออย่างยิ่ง",
        RecipientClass::Executive => "ขอแสดงความนับถืออย่างยิ่ง",
        RecipientClass::Monk => "ขอนมัสการด้วยความเคารพ",
        RecipientClass::Royal => "ขอแสดงความนับถืออย่างยิ่ง",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executive_salutation_matches_python_rule() {
        assert_eq!(lookup_salutation(&RecipientClass::Executive), "กราบเรียน");
    }

    #[test]
    fn monk_closing_matches_python_rule() {
        assert_eq!(
            lookup_closing(&RecipientClass::Monk),
            "ขอนมัสการด้วยความเคารพ"
        );
    }
}

