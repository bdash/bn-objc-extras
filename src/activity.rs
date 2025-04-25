#![allow(unused)]
use binaryninja::string::BnStrCompatible;

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub(crate) struct Config<'a> {
    pub name: &'a str,
    pub title: &'a str,
    pub description: &'a str,
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub eligibility: Eligibility,
}

impl<'a> Config<'a> {
    pub fn action(name: &'a str, title: &'a str, description: &'a str) -> Self {
        Self {
            name,
            title,
            description,
            role: Role::Action,
            eligibility: Eligibility::default(),
        }
    }

    pub fn with_eligibility(mut self, eligibility: Eligibility) -> Self {
        self.eligibility = eligibility;
        self
    }
}

unsafe impl BnStrCompatible for &Config<'_> {
    type Result = <String as BnStrCompatible>::Result;

    fn into_bytes_with_nul(self) -> Self::Result {
        serde_json::to_string_pretty(self)
            .expect("Unable to serialize Config to JSON")
            .into_bytes_with_nul()
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Action,
    Selector,
    Subflow,
    Task,
}

impl Default for Role {
    fn default() -> Self {
        Role::Action
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Eligibility {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<Auto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_once: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_once_per_session: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub predicates: Vec<Predicate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_operator: Option<PredicateLogicalOperator>,
}

impl Eligibility {
    pub fn auto() -> Self {
        Eligibility {
            auto: Some(Auto::new()),
            run_once: None,
            run_once_per_session: None,
            continuation: None,
            predicates: vec![],
            logical_operator: None,
        }
    }

    pub fn auto_with_default(value: bool) -> Self {
        Eligibility {
            auto: Some(Auto::new().default(value)),
            run_once: None,
            run_once_per_session: None,
            continuation: None,
            predicates: vec![],
            logical_operator: None,
        }
    }

    pub fn with_predicate<P: Into<Predicate>>(mut self, predicate: P) -> Self {
        self.predicates = vec![predicate.into()];
        self
    }

    pub fn matching_any_predicate(mut self, predicates: &[Predicate]) -> Self {
        self.predicates = predicates.to_vec();
        self.logical_operator = Some(PredicateLogicalOperator::Or);
        self
    }

    pub fn matching_all_predicates(mut self, predicates: &[Predicate]) -> Self {
        self.predicates = predicates.to_vec();
        self.logical_operator = Some(PredicateLogicalOperator::And);
        self
    }
}

impl Default for Eligibility {
    fn default() -> Self {
        Self::auto()
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Default)]
pub(crate) struct Auto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

impl Auto {
    pub fn new() -> Self {
        Self { default: None  }
    }

    pub fn default(mut self, value: bool) -> Self {
        self.default = Some(value);
        self
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct Predicate {
    #[serde(flatten)]
    pub predicate_type: PredicateType,
    pub operator: Operator,
    pub value: serde_json::Value,
}

impl Predicate {
    pub fn setting<I: ToString>(
        identifier: I,
        operator: Operator,
        value: impl serde::Serialize,
    ) -> Self {
        Self {
            predicate_type: PredicateType::Setting {
                identifier: identifier.to_string(),
            },
            operator,
            value: serde_json::json!(value),
        }
    }
}

pub enum ViewType<'a> {
    In(&'a [&'a str]),
    NotIn(&'a [&'a str]),
}

impl<'a> From<ViewType<'a>> for Predicate {
    fn from(predicate: ViewType) -> Self {
        match predicate {
            ViewType::In(value) => Predicate {
                predicate_type: PredicateType::ViewType,
                operator: Operator::In,
                value: serde_json::json!(value),
            },
            ViewType::NotIn(value) => Predicate {
                predicate_type: PredicateType::ViewType,
                operator: Operator::NotIn,
                value: serde_json::json!(value),
            },
        }
    }
}

pub struct Setting {
    identifier: String,
    operator: Operator,
    value: serde_json::Value,
}

impl Setting {
    pub fn new(
        identifier: impl ToString,
        operator: Operator,
        value: impl serde::Serialize,
    ) -> Self {
        Self {
            identifier: identifier.to_string(),
            operator,
            value: serde_json::json!(value),
        }
    }

    pub fn eq(identifier: impl ToString, value: impl serde::Serialize) -> Self {
        Self::new(identifier, Operator::Eq, value)
    }

    pub fn ne(identifier: impl ToString, value: impl serde::Serialize) -> Self {
        Self::new(identifier, Operator::Ne, value)
    }

    pub fn lt(identifier: impl ToString, value: impl serde::Serialize) -> Self {
        Self::new(identifier, Operator::Lt, value)
    }

    pub fn lte(identifier: impl ToString, value: impl serde::Serialize) -> Self {
        Self::new(identifier, Operator::Lte, value)
    }

    pub fn gt(identifier: impl ToString, value: impl serde::Serialize) -> Self {
        Self::new(identifier, Operator::Gt, value)
    }

    pub fn gte(identifier: impl ToString, value: impl serde::Serialize) -> Self {
        Self::new(identifier, Operator::Gte, value)
    }

    pub fn in_(identifier: impl ToString, value: impl serde::Serialize) -> Self {
        Self::new(identifier, Operator::In, value)
    }

    pub fn not_in(identifier: impl ToString, value: impl serde::Serialize) -> Self {
        Self::new(identifier, Operator::NotIn, value)
    }
}

impl From<Setting> for Predicate {
    fn from(setting: Setting) -> Self {
        Predicate {
            predicate_type: PredicateType::Setting {
                identifier: setting.identifier,
            },
            operator: setting.operator,
            value: setting.value,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum PredicateType {
    Setting { identifier: String },
    ViewType,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Copy, Clone)]
pub enum Operator {
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Lte,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Gte,
    #[serde(rename = "in")]
    In,
    #[serde(rename = "not in")]
    NotIn,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum PredicateLogicalOperator {
    And,
    Or,
}
