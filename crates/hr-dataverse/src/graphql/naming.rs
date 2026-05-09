//! Naming conventions for translating Dataverse table/column names into
//! GraphQL type and field names.
//!
//! V1 is intentionally simple: no English-language singularisation. The
//! type name is `PascalCase(table_name)`, and the query field names are
//! the table name as-is.
//!
//! Examples (table → type / query plural / query singular / mutation insert):
//! - `contacts` → `Contacts` / `contacts` / `contactsById` / `insertContacts`
//! - `company_settings` → `CompanySettings` / `companySettings` / `companySettingsById` / `insertCompanySettings`
//!
//! Authors who prefer Hasura-flavoured singular type names can simply name
//! their tables in the singular form (`contact`, `company`).

/// PascalCase: each `_`-segment is capitalised. Used for **type** names.
pub fn pascal_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut up = true;
    for ch in s.chars() {
        if ch == '_' {
            up = true;
            continue;
        }
        if up {
            out.extend(ch.to_uppercase());
            up = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// camelCase: first segment lowercase, subsequent segments capitalised.
/// Used for **field** names.
pub fn camel_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut up = false;
    let mut started = false;
    for ch in s.chars() {
        if ch == '_' {
            up = started; // first segment stays lowercase
            continue;
        }
        if up {
            out.extend(ch.to_uppercase());
            up = false;
        } else {
            out.push(ch);
        }
        started = true;
    }
    out
}

/// Reverse of [`camel_case`]: insert an underscore before each
/// uppercase letter and lowercase it. Idempotent for already
/// snake-cased strings.
pub fn snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn type_name(table: &str) -> String { pascal_case(table) }

pub fn field_list(table: &str) -> String { camel_case(table) }

pub fn field_by_id(table: &str) -> String {
    format!("{}ById", camel_case(table))
}

pub fn field_count(table: &str) -> String {
    format!("{}Count", camel_case(table))
}

pub fn input_where(table: &str) -> String {
    format!("{}Where", pascal_case(table))
}

pub fn input_order_by(table: &str) -> String {
    format!("{}OrderBy", pascal_case(table))
}

pub fn input_insert(table: &str) -> String {
    format!("{}Insert", pascal_case(table))
}

pub fn input_update(table: &str) -> String {
    format!("{}Update", pascal_case(table))
}

pub fn mutation_insert(table: &str) -> String {
    format!("insert{}", pascal_case(table))
}

pub fn mutation_update(table: &str) -> String {
    format!("update{}", pascal_case(table))
}

pub fn mutation_delete(table: &str) -> String {
    format!("delete{}", pascal_case(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_basic() {
        assert_eq!(pascal_case("contacts"), "Contacts");
        assert_eq!(pascal_case("company_settings"), "CompanySettings");
        assert_eq!(pascal_case("a"), "A");
        assert_eq!(pascal_case(""), "");
    }

    #[test]
    fn camel_basic() {
        assert_eq!(camel_case("contacts"), "contacts");
        assert_eq!(camel_case("company_settings"), "companySettings");
        assert_eq!(camel_case("a_b_c"), "aBC");
    }

    #[test]
    fn names_are_consistent() {
        assert_eq!(type_name("contacts"), "Contacts");
        assert_eq!(field_list("contacts"), "contacts");
        assert_eq!(field_by_id("contacts"), "contactsById");
        assert_eq!(input_where("contacts"), "ContactsWhere");
        assert_eq!(mutation_insert("contacts"), "insertContacts");
    }
}
