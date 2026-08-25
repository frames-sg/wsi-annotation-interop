use super::version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidatorInvocation {
    Each,
    Set,
}

#[derive(Debug, Clone)]
pub struct ValidatorSpec {
    pub name: String,
    pub command: Vec<String>,
    pub version_command: Vec<String>,
    pub validation_args: Vec<String>,
    pub invocation: ValidatorInvocation,
    pub edition: Option<String>,
    pub unsupported_markers: Vec<String>,
}

/// Return the four validators required by the full study profile.
#[must_use]
pub fn standard_validator_specs(edition: &str) -> Vec<ValidatorSpec> {
    vec![
        ValidatorSpec {
            name: "validate_iods".to_owned(),
            command: vec!["validate_iods".to_owned()],
            version_command: version::validate_iods_command(),
            validation_args: vec!["--edition".to_owned(), edition.to_owned()],
            invocation: ValidatorInvocation::Each,
            edition: Some(edition.to_owned()),
            unsupported_markers: vec!["Unknown or retired SOP Class UID".to_owned()],
        },
        ValidatorSpec {
            name: "dciodvfy".to_owned(),
            command: vec!["dciodvfy".to_owned()],
            version_command: vec!["dciodvfy".to_owned(), "-version".to_owned()],
            validation_args: Vec::new(),
            invocation: ValidatorInvocation::Each,
            edition: Some("dicom3tools embedded dictionary".to_owned()),
            unsupported_markers: Vec::new(),
        },
        ValidatorSpec {
            name: "dcentvfy".to_owned(),
            command: vec!["dcentvfy".to_owned()],
            version_command: vec!["dcentvfy".to_owned(), "-version".to_owned()],
            validation_args: Vec::new(),
            invocation: ValidatorInvocation::Set,
            edition: Some("dicom3tools embedded dictionary".to_owned()),
            unsupported_markers: Vec::new(),
        },
        ValidatorSpec {
            name: "dcm2json".to_owned(),
            command: vec!["dcm2json".to_owned()],
            version_command: vec!["dcm2json".to_owned(), "--version".to_owned()],
            validation_args: Vec::new(),
            invocation: ValidatorInvocation::Each,
            edition: Some("DCMTK embedded dictionary".to_owned()),
            unsupported_markers: Vec::new(),
        },
    ]
}
