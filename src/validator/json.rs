#![cfg(feature = "std")]

use crate::{
  ast::*,
  token::{self, Token},
  visitor::{self, *},
};
use chrono::{TimeZone, Utc};
use serde_json::Value;
use std::{collections::HashMap, convert::TryFrom, fmt};

use super::*;

/// JSON validation Result
pub type Result = std::result::Result<(), Error>;

/// JSON validation error
#[derive(Debug)]
pub enum Error {
  /// Zero or more validation errors
  Validation(Vec<ValidationError>),
  /// JSON parsing error
  JSONParsing(serde_json::Error),
  /// CDDL parsing error
  CDDLParsing(String),
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Error::Validation(errors) => {
        let mut error_str = String::new();
        for e in errors.iter() {
          error_str.push_str(&format!("{}\n", e));
        }
        write!(f, "{}", error_str)
      }
      Error::JSONParsing(error) => write!(f, "error parsing JSON: {}", error),
      Error::CDDLParsing(error) => write!(f, "error parsing CDDL: {}", error),
    }
  }
}

impl std::error::Error for Error {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Error::JSONParsing(error) => Some(error),
      _ => None,
    }
  }
}

/// JSON validation error
#[derive(Clone, Debug)]
pub struct ValidationError {
  /// Error message
  pub reason: String,
  /// Location in CDDL where error occurred
  pub cddl_location: String,
  /// Location in JSON (in JSONPointer notation) where error occurred
  pub json_location: String,
  /// Whether or not the error is associated with multiple type choices
  pub is_multi_type_choice: bool,
  /// Whether or not the error is associated with multiple group choices
  pub is_multi_group_choice: bool,
  /// Whether or not the error is associated with a group to choice enumeration
  pub is_group_to_choice_enum: bool,
  /// Error is associated with a type/group name group entry
  pub type_group_name_entry: Option<String>,
}

impl fmt::Display for ValidationError {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    let mut error_str = String::from("error validating");
    if self.is_multi_group_choice {
      error_str.push_str(" group choice");
    }
    if self.is_multi_type_choice {
      error_str.push_str(" type choice");
    }
    if self.is_group_to_choice_enum {
      error_str.push_str(" type choice in group to choice enumeration");
    }
    if let Some(entry) = &self.type_group_name_entry {
      error_str.push_str(&format!(" group entry associated with rule \"{}\"", entry));
    }

    write!(
      f,
      "{} at cddl location \"{}\" and JSON location {}: {}",
      error_str, self.cddl_location, self.json_location, self.reason
    )
  }
}

impl std::error::Error for ValidationError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    None
  }
}

impl ValidationError {
  fn from_validator(jv: &JSONValidator, reason: String) -> Self {
    ValidationError {
      cddl_location: jv.cddl_location.clone(),
      json_location: jv.json_location.clone(),
      reason,
      is_multi_type_choice: jv.is_multi_type_choice,
      is_group_to_choice_enum: jv.is_group_to_choice_enum,
      type_group_name_entry: jv.type_group_name_entry.map(|e| e.to_string()),
      is_multi_group_choice: jv.is_multi_group_choice,
    }
  }
}

/// JSON validator type
pub struct JSONValidator<'a> {
  cddl: &'a CDDL<'a>,
  json: Value,
  errors: Vec<ValidationError>,
  cddl_location: String,
  json_location: String,
  // Occurrence indicator detected in current state of AST evaluation
  occurrence: Option<Occur>,
  // Current group entry index detected in current state of AST evaluation
  group_entry_idx: Option<usize>,
  // JSON object value hoisted from previous state of AST evaluation
  object_value: Option<Value>,
  // Is member key detected in current state of AST evaluation
  is_member_key: bool,
  // Is a cut detected in current state of AST evaluation
  is_cut_present: bool,
  // Str value of cut detected in current state of AST evaluation
  cut_value: Option<&'a str>,
  // Validate the generic rule given by str ident in current state of AST
  // evaluation
  eval_generic_rule: Option<&'a str>,
  // Aggregation of generic rules
  generic_rules: Vec<GenericRule<'a>>,
  // Control operator token detected in current state of AST evaluation
  ctrl: Option<token::Token<'a>>,
  // Is a group to choice enumeration detected in current state of AST
  // evaluation
  is_group_to_choice_enum: bool,
  // Are 2 or more type choices detected in current state of AST evaluation
  is_multi_type_choice: bool,
  // Are 2 or more group choices detected in current state of AST evaluation
  is_multi_group_choice: bool,
  // Type/group name entry detected in current state of AST evaluation. Used
  // only for providing more verbose error messages
  type_group_name_entry: Option<&'a str>,
  // Whether or not to advance to the next group entry if member key validation
  // fails as detected during the current state of AST evaluation
  advance_to_next_entry: bool,
  is_ctrl_map_equality: bool,
  entry_counts: Option<Vec<EntryCount>>,
  // Collect map entry keys that have already been validated
  validated_keys: Option<Vec<String>>,
  // Collect map entry values that have yet to be validated
  values_to_validate: Option<Vec<Value>>,
  // Collect valid array indices when entries are type choices
  valid_array_items: Option<Vec<usize>>,
  // Collect invalid array item errors where the key is the index of the invalid
  // array item
  array_errors: Option<HashMap<usize, Vec<ValidationError>>>,
}

#[derive(Clone, Debug)]
struct GenericRule<'a> {
  name: &'a str,
  params: Vec<&'a str>,
  args: Vec<Type1<'a>>,
}

impl<'a> JSONValidator<'a> {
  /// New JSONValidation from CDDL AST and JSON value
  pub fn new(cddl: &'a CDDL<'a>, json: Value) -> Self {
    JSONValidator {
      cddl,
      json,
      errors: Vec::default(),
      cddl_location: String::new(),
      json_location: String::new(),
      occurrence: None,
      group_entry_idx: None,
      object_value: None,
      is_member_key: false,
      is_cut_present: false,
      cut_value: None,
      eval_generic_rule: None,
      generic_rules: Vec::new(),
      ctrl: None,
      is_group_to_choice_enum: false,
      is_multi_type_choice: false,
      is_multi_group_choice: false,
      type_group_name_entry: None,
      advance_to_next_entry: false,
      is_ctrl_map_equality: false,
      entry_counts: None,
      validated_keys: None,
      values_to_validate: None,
      valid_array_items: None,
      array_errors: None,
    }
  }

  /// Validate
  pub fn validate(&mut self) -> std::result::Result<(), Error> {
    for r in self.cddl.rules.iter() {
      // First type rule is root
      if let Rule::Type { rule, .. } = r {
        if rule.generic_params.is_none() {
          self
            .visit_type_rule(rule)
            .map_err(|e| Error::Validation(vec![e]))?;
          break;
        }
      }
    }

    if !self.errors.is_empty() {
      return Err(Error::Validation(self.errors.clone()));
    }

    Ok(())
  }

  fn add_error(&mut self, reason: String) {
    self.errors.push(ValidationError {
      reason,
      cddl_location: self.cddl_location.clone(),
      json_location: self.json_location.clone(),
      is_multi_type_choice: self.is_multi_type_choice,
      is_multi_group_choice: self.is_multi_group_choice,
      is_group_to_choice_enum: self.is_group_to_choice_enum,
      type_group_name_entry: self.type_group_name_entry.map(|e| e.to_string()),
    });
  }
}

impl<'a> Visitor<'a, ValidationError> for JSONValidator<'a> {
  fn visit_type_rule(&mut self, tr: &TypeRule<'a>) -> visitor::Result<ValidationError> {
    if let Some(gp) = &tr.generic_params {
      if let Some(gr) = self
        .generic_rules
        .iter_mut()
        .find(|r| r.name == tr.name.ident)
      {
        gr.params = gp.params.iter().map(|p| p.param.ident).collect();
      } else {
        self.generic_rules.push(GenericRule {
          name: tr.name.ident,
          params: gp.params.iter().map(|p| p.param.ident).collect(),
          args: vec![],
        });
      }
    }

    let error_count = self.errors.len();

    for t in type_choice_alternates_from_ident(self.cddl, &tr.name) {
      let cur_errors = self.errors.len();
      self.visit_type(t)?;
      if self.errors.len() == cur_errors {
        for _ in 0..self.errors.len() - error_count {
          self.errors.pop();
        }

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_group_rule(&mut self, gr: &GroupRule<'a>) -> visitor::Result<ValidationError> {
    if let Some(gp) = &gr.generic_params {
      if let Some(gr) = self
        .generic_rules
        .iter_mut()
        .find(|r| r.name == gr.name.ident)
      {
        gr.params = gp.params.iter().map(|p| p.param.ident).collect();
      } else {
        self.generic_rules.push(GenericRule {
          name: gr.name.ident,
          params: gp.params.iter().map(|p| p.param.ident).collect(),
          args: vec![],
        });
      }
    }

    let error_count = self.errors.len();

    for ge in group_choice_alternates_from_ident(self.cddl, &gr.name) {
      let cur_errors = self.errors.len();
      self.visit_group_entry(ge)?;
      if self.errors.len() == cur_errors {
        for _ in 0..self.errors.len() - error_count {
          self.errors.pop();
        }

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_type(&mut self, t: &Type<'a>) -> visitor::Result<ValidationError> {
    if t.type_choices.len() > 1 {
      self.is_multi_type_choice = true;
    }

    let initial_error_count = self.errors.len();
    for type_choice in t.type_choices.iter() {
      // If validating an array whose elements are type choices (i.e. [ 1* tstr
      // / integer ]), collect all errors and filter after the fact
      if matches!(self.json, Value::Array(_)) {
        let error_count = self.errors.len();
        self.visit_type_choice(type_choice)?;
        if self.errors.len() == error_count {
          // Disregard invalid type choice validation errors if one of the
          // choices validates successfully
          let type_choice_error_count = self.errors.len() - initial_error_count;
          if type_choice_error_count > 0 {
            for _ in 0..type_choice_error_count {
              self.errors.pop();
            }
          }
        }

        continue;
      }

      let error_count = self.errors.len();
      self.visit_type_choice(type_choice)?;

      if self.errors.len() == error_count {
        // Disregard invalid type choice validation errors if one of the
        // choices validates successfully
        let type_choice_error_count = self.errors.len() - initial_error_count;
        if type_choice_error_count > 0 {
          for _ in 0..type_choice_error_count {
            self.errors.pop();
          }
        }

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_group(&mut self, g: &Group<'a>) -> visitor::Result<ValidationError> {
    if g.group_choices.len() > 1 {
      self.is_multi_group_choice = true;
    }

    // Map equality/inequality validation
    if self.is_ctrl_map_equality {
      if let Some(t) = &self.ctrl {
        if let Value::Object(o) = &self.json {
          let mut entry_counts = Vec::new();
          for gc in g.group_choices.iter() {
            let count = entry_counts_from_group_choice(self.cddl, gc);
            entry_counts.push(count);
          }
          let len = o.len();
          if let Token::EQ = t {
            if !validate_entry_count(&entry_counts, len) {
              for ec in entry_counts.iter() {
                if let Some(occur) = &ec.entry_occurrence {
                  self.add_error(format!(
                    "map equality error. expected object with number of entries per occurrence {}",
                    occur,
                  ));
                } else {
                  self.add_error(format!(
                    "map equality error, expected object with length {}, got {}",
                    ec.count, len
                  ));
                }
              }
              return Ok(());
            }
          } else if let Token::NE = t {
            if !validate_entry_count(&entry_counts, len) {
              for ec in entry_counts.iter() {
                if let Some(occur) = &ec.entry_occurrence {
                  self.add_error(format!(
                    "map inequality error. expected object with number of entries not per occurrence {}",
                    occur,
                  ));
                } else {
                  self.add_error(format!(
                    "map inequality error, expected object not with length {}, got {}",
                    ec.count, len
                  ));
                }
              }
              return Ok(());
            }
          }
        }
      }
    }

    self.is_ctrl_map_equality = false;

    let initial_error_count = self.errors.len();
    for group_choice in g.group_choices.iter() {
      let error_count = self.errors.len();
      self.visit_group_choice(group_choice)?;
      if self.errors.len() == error_count {
        // Disregard invalid group choice validation errors if one of the
        // choices validates successfully
        let group_choice_error_count = self.errors.len() - initial_error_count;
        if group_choice_error_count > 0 {
          for _ in 0..group_choice_error_count {
            self.errors.pop();
          }
        }

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_group_choice(&mut self, gc: &GroupChoice<'a>) -> visitor::Result<ValidationError> {
    if self.is_group_to_choice_enum {
      let initial_error_count = self.errors.len();
      for tc in type_choices_from_group_choice(self.cddl, gc).iter() {
        let error_count = self.errors.len();
        self.visit_type_choice(tc)?;
        if self.errors.len() == error_count {
          let type_choice_error_count = self.errors.len() - initial_error_count;
          if type_choice_error_count > 0 {
            for _ in 0..type_choice_error_count {
              self.errors.pop();
            }
          }
          return Ok(());
        }
      }

      return Ok(());
    }

    for (idx, ge) in gc.group_entries.iter().enumerate() {
      self.group_entry_idx = Some(idx);

      self.visit_group_entry(&ge.0)?;
    }

    Ok(())
  }

  fn visit_range(
    &mut self,
    lower: &Type2,
    upper: &Type2,
    is_inclusive: bool,
  ) -> visitor::Result<ValidationError> {
    if let Value::Array(a) = &self.json {
      match validate_array_occurrence(
        self.occurrence.as_ref().take(),
        self.entry_counts.as_ref().map(|ec| &ec[..]),
        a,
      ) {
        Ok((iter_items, allow_empty_array)) => {
          if iter_items {
            for (idx, v) in a.iter().enumerate() {
              if let Some(indices) = &self.valid_array_items {
                if self.is_multi_type_choice && indices.contains(&idx) {
                  continue;
                }
              }

              let mut jv = JSONValidator::new(self.cddl, v.clone());
              jv.generic_rules = self.generic_rules.clone();
              jv.eval_generic_rule = self.eval_generic_rule;
              jv.ctrl = self.ctrl.clone();
              jv.is_multi_type_choice = self.is_multi_type_choice;
              jv.json_location
                .push_str(&format!("{}/{}", self.json_location, idx));

              jv.visit_range(lower, upper, is_inclusive)?;

              if self.is_multi_type_choice && jv.errors.is_empty() {
                if let Some(indices) = &mut self.valid_array_items {
                  indices.push(idx);
                } else {
                  self.valid_array_items = Some(vec![idx]);
                }
                continue;
              }

              if let Some(errors) = &mut self.array_errors {
                if let Some(error) = errors.get_mut(&idx) {
                  error.append(&mut jv.errors);
                } else {
                  errors.insert(idx, jv.errors);
                }
              } else {
                let mut errors = HashMap::new();
                errors.insert(idx, jv.errors);
                self.array_errors = Some(errors)
              }
            }
          } else if let Some(idx) = self.group_entry_idx.take() {
            if let Some(v) = a.get(idx) {
              let mut jv = JSONValidator::new(self.cddl, v.clone());
              jv.generic_rules = self.generic_rules.clone();
              jv.eval_generic_rule = self.eval_generic_rule;
              jv.ctrl = self.ctrl.clone();
              jv.is_multi_type_choice = self.is_multi_type_choice;
              jv.json_location
                .push_str(&format!("{}/{}", self.json_location, idx));

              jv.visit_range(lower, upper, is_inclusive)?;

              self.errors.append(&mut jv.errors);
            } else if !allow_empty_array {
              self.add_error(format!("expected array item at index {}", idx));
            }
          } else {
            self.add_error(format!(
              "expected range lower {} upper {} inclusive {}, got {}",
              lower, upper, is_inclusive, self.json
            ));
          }
        }
        Err(errors) => {
          for e in errors.into_iter() {
            self.add_error(e);
          }
        }
      }

      return Ok(());
    }

    match lower {
      Type2::IntValue { value: l, .. } => match upper {
        Type2::IntValue { value: u, .. } => {
          let error_str = if is_inclusive {
            format!(
              "expected integer to be in range {} <= value <= {}, got {}",
              l, u, self.json
            )
          } else {
            format!(
              "expected integer to be in range {} < value < {}, got {}",
              l, u, self.json
            )
          };

          match &self.json {
            Value::Number(n) => {
              if let Some(i) = n.as_i64() {
                if is_inclusive {
                  if i < *l as i64 || i > *u as i64 {
                    self.add_error(error_str);
                  } else {
                    return Ok(());
                  }
                } else if i <= *l as i64 || i >= *u as i64 {
                  self.add_error(error_str);
                  return Ok(());
                } else {
                  return Ok(());
                }
              } else {
                self.add_error(error_str);
                return Ok(());
              }
            }
            _ => {
              self.add_error(error_str);
              return Ok(());
            }
          }
        }
        Type2::UintValue { value: u, .. } => {
          let error_str = if is_inclusive {
            format!(
              "expected integer to be in range {} <= value <= {}, got {}",
              l, u, self.json
            )
          } else {
            format!(
              "expected integer to be in range {} < value < {}, got {}",
              l, u, self.json
            )
          };

          match &self.json {
            Value::Number(n) => {
              if let Some(i) = n.as_i64() {
                if is_inclusive {
                  if i < *l as i64 || i > *u as i64 {
                    self.add_error(error_str);
                  } else {
                    return Ok(());
                  }
                } else if i <= *l as i64 || i >= *u as i64 {
                  self.add_error(error_str);
                  return Ok(());
                } else {
                  return Ok(());
                }
              } else {
                self.add_error(error_str);
                return Ok(());
              }
            }
            _ => {
              self.add_error(error_str);
              return Ok(());
            }
          }
        }
        _ => {
          self.add_error(format!(
            "invalid cddl range. upper value must be an integer type. got {}",
            upper
          ));
          return Ok(());
        }
      },
      Type2::UintValue { value: l, .. } => match upper {
        Type2::UintValue { value: u, .. } => {
          let error_str = if is_inclusive {
            format!(
              "expected uint to be in range {} <= value <= {}, got {}",
              l, u, self.json
            )
          } else {
            format!(
              "expected uint to be in range {} < value < {}, got {}",
              l, u, self.json
            )
          };

          match &self.json {
            Value::Number(n) => {
              if let Some(i) = n.as_u64() {
                if is_inclusive {
                  if i < *l as u64 || i > *u as u64 {
                    self.add_error(error_str);
                  } else {
                    return Ok(());
                  }
                } else if i <= *l as u64 || i >= *u as u64 {
                  self.add_error(error_str);
                  return Ok(());
                } else {
                  return Ok(());
                }
              } else {
                self.add_error(error_str);
                return Ok(());
              }
            }
            Value::String(s) => match self.ctrl {
              Some(Token::SIZE) => {
                let len = s.len();
                let s = s.clone();
                if is_inclusive {
                  if s.len() < *l || s.len() > *u {
                    self.add_error(format!(
                      "expected \"{}\" string length to be in the range {} <= value <= {}, got {}",
                      s, l, u, len
                    ));
                    return Ok(());
                  } else {
                    return Ok(());
                  }
                } else if s.len() <= *l || s.len() >= *u {
                  self.add_error(format!(
                    "expected \"{}\" string length to be in the range {} < value < {}, got {}",
                    s, l, u, len
                  ));
                  return Ok(());
                }
              }
              _ => {
                self.add_error("string value cannot be validated against a range without the .size control operator".to_string());
                return Ok(());
              }
            },
            _ => {
              self.add_error(error_str);
              return Ok(());
            }
          }
        }
        _ => {
          self.add_error(format!(
            "invalid cddl range. upper value must be a uint type. got {}",
            upper
          ));
          return Ok(());
        }
      },
      Type2::FloatValue { value: l, .. } => match upper {
        Type2::FloatValue { value: u, .. } => {
          let error_str = if is_inclusive {
            format!(
              "expected float to be in range {} <= value <= {}, got {}",
              l, u, self.json
            )
          } else {
            format!(
              "expected float to be in range {} < value < {}, got {}",
              l, u, self.json
            )
          };

          match &self.json {
            Value::Number(n) => {
              if let Some(f) = n.as_f64() {
                if is_inclusive {
                  if f < *l as f64 || f > *u as f64 {
                    self.add_error(error_str);
                  } else {
                    return Ok(());
                  }
                } else if f <= *l as f64 || f >= *u as f64 {
                  self.add_error(error_str);
                  return Ok(());
                } else {
                  return Ok(());
                }
              } else {
                self.add_error(error_str);
                return Ok(());
              }
            }
            _ => {
              self.add_error(error_str);
              return Ok(());
            }
          }
        }
        _ => {
          self.add_error(format!(
            "invalid cddl range. upper value must be a float type. got {}",
            upper
          ));
          return Ok(());
        }
      },
      _ => {
        self.add_error(
          "invalid cddl range. upper and lower values must be either integers or floats"
            .to_string(),
        );

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_control_operator(
    &mut self,
    target: &Type2<'a>,
    ctrl: &str,
    controller: &Type2<'a>,
  ) -> visitor::Result<ValidationError> {
    match lookup_control_from_str(ctrl) {
      t @ Some(Token::EQ) => {
        match target {
          Type2::Typename { ident, .. } => {
            if is_ident_string_data_type(self.cddl, ident)
              || is_ident_numeric_data_type(self.cddl, ident)
            {
              return self.visit_type2(controller);
            }
          }
          Type2::Array { group, .. } => {
            if let Value::Array(_) = &self.json {
              let mut entry_counts = Vec::new();
              for gc in group.group_choices.iter() {
                let count = entry_counts_from_group_choice(self.cddl, gc);
                entry_counts.push(count);
              }
              self.entry_counts = Some(entry_counts);
              self.visit_type2(controller)?;
              self.entry_counts = None;
              return Ok(());
            }
          }
          Type2::Map { .. } => {
            if let Value::Object(_) = &self.json {
              self.ctrl = t;
              self.is_ctrl_map_equality = true;
              self.visit_type2(controller)?;
              self.ctrl = None;
              self.is_ctrl_map_equality = false;
              return Ok(());
            }
          }
          _ => self.add_error(format!(
            "target for .eq operator must be a string, numerical, array or map data type, got {}",
            target
          )),
        }
        Ok(())
      }
      t @ Some(Token::NE) => {
        match target {
          Type2::Typename { ident, .. } => {
            if is_ident_string_data_type(self.cddl, ident)
              || is_ident_numeric_data_type(self.cddl, ident)
            {
              self.ctrl = t;
              self.visit_type2(controller)?;
              self.ctrl = None;
              return Ok(());
            }
          }
          Type2::Array { .. } => {
            if let Value::Array(_) = &self.json {
              self.ctrl = t;
              self.visit_type2(controller)?;
              self.ctrl = None;
              return Ok(());
            }
          }
          Type2::Map { .. } => {
            if let Value::Object(_) = &self.json {
              self.ctrl = t;
              self.is_ctrl_map_equality = true;
              self.visit_type2(controller)?;
              self.ctrl = None;
              self.is_ctrl_map_equality = false;
              return Ok(());
            }
          }
          _ => self.add_error(format!(
            "target for .ne operator must be a string, numerical, array or map data type, got {}",
            target
          )),
        }
        Ok(())
      }
      t @ Some(Token::LT) | t @ Some(Token::GT) | t @ Some(Token::GE) | t @ Some(Token::LE) => {
        match target {
          Type2::Typename { ident, .. } if is_ident_numeric_data_type(self.cddl, ident) => {
            self.ctrl = t;
            self.visit_type2(controller)?;
            self.ctrl = None;
            Ok(())
          }
          _ => {
            self.add_error(format!(
              "target for .lt, .gt, .ge or .le operator must be a numerical data type, got {}",
              target
            ));
            Ok(())
          }
        }
      }
      t @ Some(Token::SIZE) => match target {
        Type2::Typename { ident, .. }
          if is_ident_string_data_type(self.cddl, ident)
            || is_ident_uint_data_type(self.cddl, ident) =>
        {
          self.ctrl = t;
          self.visit_type2(controller)?;
          self.ctrl = None;
          Ok(())
        }
        _ => {
          self.add_error(format!(
            "target for .size must a string or uint data type, got {}",
            target
          ));
          Ok(())
        }
      },
      t @ Some(Token::AND) => {
        self.ctrl = t;
        self.visit_type2(target)?;
        self.visit_type2(controller)?;
        self.ctrl = None;
        Ok(())
      }
      t @ Some(Token::WITHIN) => {
        self.ctrl = t;
        let error_count = self.errors.len();
        self.visit_type2(target)?;
        let no_errors = self.errors.len() == error_count;
        self.visit_type2(controller)?;
        if no_errors && self.errors.len() > error_count {
          for _ in 0..self.errors.len() - error_count {
            self.errors.pop();
          }

          self.add_error(format!(
            "expected type {} .within type {}, got {}",
            target, controller, self.json,
          ));
        }

        self.ctrl = None;

        Ok(())
      }
      t @ Some(Token::DEFAULT) => {
        self.ctrl = t;
        let error_count = self.errors.len();
        self.visit_type2(target)?;
        if self.errors.len() != error_count {
          if let Some(Occur::Optional(_)) = self.occurrence.take() {
            self.add_error(format!(
              "expected default value {}, got {}",
              controller, self.json
            ));
          }
        }
        self.ctrl = None;
        Ok(())
      }
      t @ Some(Token::REGEXP) | t @ Some(Token::PCRE) => {
        self.ctrl = t;
        match target {
          Type2::Typename { ident, .. } if is_ident_string_data_type(self.cddl, ident) => {
            match self.json {
              Value::String(_) | Value::Array(_) => self.visit_type2(controller)?,
              _ => self.add_error(format!(
                ".regexp/.pcre control can only be matched against JSON string, got {}",
                self.json
              )),
            }
          }
          _ => self.add_error(format!(
            ".regexp/.pcre control can only be matched against string data type, got {}",
            target
          )),
        }
        self.ctrl = None;

        Ok(())
      }
      _ => {
        self.add_error(format!("unsupported control operator {}", ctrl));
        Ok(())
      }
    }
  }

  fn visit_type2(&mut self, t2: &Type2<'a>) -> visitor::Result<ValidationError> {
    match t2 {
      Type2::TextValue { value, .. } => self.visit_value(&token::Value::TEXT(value)),
      Type2::Map { group, .. } => match &self.json {
        Value::Object(o) => {
          let o = o.keys().cloned().collect::<Vec<_>>();

          self.visit_group(group)?;

          if self.values_to_validate.is_none() {
            for k in o.into_iter() {
              if let Some(keys) = &self.validated_keys {
                if !keys.contains(&k) {
                  self.add_error(format!("unexpected key {:?}", k));
                }
              }
            }
          }

          self.is_cut_present = false;
          self.cut_value = None;
          Ok(())
        }
        Value::Array(a) => {
          // Member keys are annotation only in an array context
          if self.is_member_key {
            return Ok(());
          }

          match validate_array_occurrence(
            self.occurrence.as_ref().take(),
            self.entry_counts.as_ref().map(|ec| &ec[..]),
            a,
          ) {
            Ok((iter_items, allow_empty_array)) => {
              if iter_items {
                for (idx, v) in a.iter().enumerate() {
                  if let Some(indices) = &self.valid_array_items {
                    if self.is_multi_type_choice && indices.contains(&idx) {
                      continue;
                    }
                  }

                  let mut jv = JSONValidator::new(self.cddl, v.clone());
                  jv.generic_rules = self.generic_rules.clone();
                  jv.eval_generic_rule = self.eval_generic_rule;
                  jv.ctrl = self.ctrl.clone();
                  jv.is_multi_type_choice = self.is_multi_type_choice;
                  jv.json_location
                    .push_str(&format!("{}/{}", self.json_location, idx));

                  jv.visit_group(group)?;

                  if self.is_multi_type_choice && jv.errors.is_empty() {
                    if let Some(indices) = &mut self.valid_array_items {
                      indices.push(idx);
                    } else {
                      self.valid_array_items = Some(vec![idx]);
                    }
                    continue;
                  }

                  if let Some(errors) = &mut self.array_errors {
                    if let Some(error) = errors.get_mut(&idx) {
                      error.append(&mut jv.errors);
                    } else {
                      errors.insert(idx, jv.errors);
                    }
                  } else {
                    let mut errors = HashMap::new();
                    errors.insert(idx, jv.errors);
                    self.array_errors = Some(errors)
                  }
                }
              } else if let Some(idx) = self.group_entry_idx.take() {
                if let Some(v) = a.get(idx) {
                  let mut jv = JSONValidator::new(self.cddl, v.clone());
                  jv.generic_rules = self.generic_rules.clone();
                  jv.eval_generic_rule = self.eval_generic_rule;
                  jv.ctrl = self.ctrl.clone();
                  jv.is_multi_type_choice = self.is_multi_type_choice;
                  jv.json_location
                    .push_str(&format!("{}/{}", self.json_location, idx));

                  jv.visit_group(group)?;

                  self.errors.append(&mut jv.errors);
                } else if !allow_empty_array {
                  self.add_error(format!("expected map object {} at index {}", group, idx));
                }
              } else {
                self.add_error(format!("expected map object {}, got {}", group, self.json));
              }
            }
            Err(errors) => {
              for e in errors.into_iter() {
                self.add_error(e);
              }
            }
          }

          Ok(())
        }
        _ => {
          self.add_error(format!("expected map object {}, got {}", t2, self.json));
          Ok(())
        }
      },
      Type2::Array { group, .. } => match &self.json {
        Value::Array(a) => {
          if group.group_choices.len() == 1
            && group.group_choices[0].group_entries.is_empty()
            && !a.is_empty()
            && !matches!(self.ctrl, Some(Token::NE))
          {
            self.add_error(format!("expected empty array, got {}", self.json));
            return Ok(());
          }

          let mut entry_counts = Vec::new();
          for gc in group.group_choices.iter() {
            let count = entry_counts_from_group_choice(self.cddl, gc);
            entry_counts.push(count);
          }

          self.entry_counts = Some(entry_counts);
          self.visit_group(group)?;
          self.entry_counts = None;

          if let Some(errors) = &mut self.array_errors {
            if let Some(indices) = &self.valid_array_items {
              for idx in indices.iter() {
                errors.remove(idx);
              }
            }

            for mut error in errors.values_mut() {
              self.errors.append(&mut error);
            }
          }

          self.valid_array_items = None;
          self.array_errors = None;

          Ok(())
        }
        _ => {
          self.add_error(format!("expected array type, got {}", self.json));
          Ok(())
        }
      },
      Type2::ChoiceFromGroup {
        ident,
        generic_args,
        ..
      } => {
        if let Some(ga) = generic_args {
          if let Some(rule) = rule_from_ident(self.cddl, ident) {
            if let Some(gr) = self
              .generic_rules
              .iter_mut()
              .find(|gr| gr.name == ident.ident)
            {
              for arg in ga.args.iter() {
                gr.args.push((*arg.arg).clone());
              }
            } else if let Some(params) = generic_params_from_rule(rule) {
              self.generic_rules.push(GenericRule {
                name: ident.ident,
                params,
                args: ga.args.iter().cloned().map(|arg| *arg.arg).collect(),
              });
            }

            let mut jv = JSONValidator::new(self.cddl, self.json.clone());
            jv.generic_rules = self.generic_rules.clone();
            jv.eval_generic_rule = Some(ident.ident);
            jv.is_group_to_choice_enum = true;
            jv.is_multi_type_choice = self.is_multi_type_choice;
            jv.visit_rule(rule)?;

            self.errors.append(&mut jv.errors);

            return Ok(());
          }
        }

        if group_rule_from_ident(self.cddl, ident).is_none() {
          self.add_error(format!(
            "rule {} must be a group rule to turn it into a choice",
            ident
          ));
          return Ok(());
        }

        self.is_group_to_choice_enum = true;
        self.visit_identifier(ident)?;
        self.is_group_to_choice_enum = false;

        Ok(())
      }
      Type2::ChoiceFromInlineGroup { group, .. } => {
        self.is_group_to_choice_enum = true;
        self.visit_group(group)?;
        self.is_group_to_choice_enum = false;
        Ok(())
      }
      Type2::Typename {
        ident,
        generic_args,
        ..
      } => {
        if let Some(ga) = generic_args {
          if let Some(rule) = rule_from_ident(self.cddl, ident) {
            if let Some(gr) = self
              .generic_rules
              .iter_mut()
              .find(|gr| gr.name == ident.ident)
            {
              for arg in ga.args.iter() {
                gr.args.push((*arg.arg).clone());
              }
            } else if let Some(params) = generic_params_from_rule(rule) {
              self.generic_rules.push(GenericRule {
                name: ident.ident,
                params,
                args: ga.args.iter().cloned().map(|arg| *arg.arg).collect(),
              });
            }

            let mut jv = JSONValidator::new(self.cddl, self.json.clone());
            jv.generic_rules = self.generic_rules.clone();
            jv.eval_generic_rule = Some(ident.ident);
            jv.is_multi_type_choice = self.is_multi_type_choice;
            jv.visit_rule(rule)?;

            self.errors.append(&mut jv.errors);

            return Ok(());
          }
        }

        self.visit_identifier(ident)
      }
      Type2::IntValue { value, .. } => self.visit_value(&token::Value::INT(*value)),
      Type2::UintValue { value, .. } => self.visit_value(&token::Value::UINT(*value)),
      Type2::FloatValue { value, .. } => self.visit_value(&token::Value::FLOAT(*value)),
      Type2::ParenthesizedType { pt, .. } => self.visit_type(pt),
      Type2::Unwrap {
        ident,
        generic_args,
        ..
      } => {
        if let Some(tag) = tag_from_token(&lookup_ident(ident.ident)) {
          // Per
          // https://github.com/w3c/did-spec-registries/pull/138#issuecomment-719739215,
          // strip tag and validate underlying type
          if let Type2::TaggedData { t, .. } = tag {
            return self.visit_type(&t);
          }
        }

        if let Some(ga) = generic_args {
          if let Some(rule) = unwrap_rule_from_ident(self.cddl, ident) {
            if let Some(gr) = self
              .generic_rules
              .iter_mut()
              .find(|gr| gr.name == ident.ident)
            {
              for arg in ga.args.iter() {
                gr.args.push((*arg.arg).clone());
              }
            } else if let Some(params) = generic_params_from_rule(rule) {
              self.generic_rules.push(GenericRule {
                name: ident.ident,
                params,
                args: ga.args.iter().cloned().map(|arg| *arg.arg).collect(),
              });
            }

            let mut jv = JSONValidator::new(self.cddl, self.json.clone());
            jv.generic_rules = self.generic_rules.clone();
            jv.eval_generic_rule = Some(ident.ident);
            jv.is_multi_type_choice = self.is_multi_type_choice;
            jv.visit_rule(rule)?;

            self.errors.append(&mut jv.errors);

            return Ok(());
          }
        }

        if let Some(rule) = unwrap_rule_from_ident(self.cddl, ident) {
          return self.visit_rule(rule);
        }

        self.add_error(format!(
          "cannot unwrap identifier {}, rule not found",
          ident
        ));

        Ok(())
      }
      Type2::Any(_) => Ok(()),
      _ => {
        self.add_error(format!(
          "unsupported data type for validating JSON, got {}",
          t2
        ));
        Ok(())
      }
    }
  }

  fn visit_identifier(&mut self, ident: &Identifier<'a>) -> visitor::Result<ValidationError> {
    if let Some(name) = self.eval_generic_rule {
      if let Some(gr) = self
        .generic_rules
        .iter()
        .cloned()
        .find(|gr| gr.name == name)
      {
        for (idx, gp) in gr.params.iter().enumerate() {
          if *gp == ident.ident {
            if let Some(arg) = gr.args.get(idx) {
              return self.visit_type1(arg);
            }
          }
        }
      }
    }

    if let Some(r) = rule_from_ident(self.cddl, ident) {
      return self.visit_rule(r);
    }

    if is_ident_any_type(self.cddl, ident) {
      return Ok(());
    }

    match &self.json {
      Value::Null if is_ident_null_data_type(self.cddl, ident) => Ok(()),
      Value::Bool(b) => {
        if is_ident_bool_data_type(self.cddl, ident) {
          return Ok(());
        }

        if ident_matches_bool_value(self.cddl, ident, *b) {
          return Ok(());
        }

        self.add_error(format!("expected type {}, got {}", ident, self.json));
        Ok(())
      }
      Value::Number(n) => {
        if is_ident_uint_data_type(self.cddl, ident) && n.is_u64() {
          return Ok(());
        } else if is_ident_nint_data_type(self.cddl, ident) {
          if let Some(n) = n.as_i64() {
            if n.is_negative() {
              return Ok(());
            }
          }
        } else if is_ident_time_data_type(self.cddl, ident) {
          if let Some(n) = n.as_i64() {
            if let chrono::LocalResult::None = Utc.timestamp_millis_opt(n * 1000) {
              self.add_error(format!(
                "expected time data type, invalid UNIX timestamp {}",
                n,
              ));
            }

            return Ok(());
          } else if let Some(n) = n.as_f64() {
            // truncates fractional milliseconds when validating
            if let chrono::LocalResult::None = Utc.timestamp_millis_opt((n * 1000f64) as i64) {
              self.add_error(format!(
                "expected time data type, invalid UNIX timestamp {}",
                n,
              ));
            }
          }
        } else if (is_ident_integer_data_type(self.cddl, ident) && n.is_i64())
          || (is_ident_float_data_type(self.cddl, ident) && n.is_f64())
        {
          return Ok(());
        }

        self.add_error(format!("expected type {}, got {}", ident, self.json));
        Ok(())
      }
      Value::String(s) => {
        if is_ident_uri_data_type(self.cddl, ident) {
          if let Err(e) = uriparse::URI::try_from(&**s) {
            self.add_error(format!("expected URI data type, decoding error: {}", e));
          }
        } else if is_ident_b64url_data_type(self.cddl, ident) {
          if let Err(e) = base64_url::decode(s) {
            self.add_error(format!(
              "expected base64 URL data type, decoding error: {}",
              e
            ));
          }
        } else if is_ident_tdate_data_type(self.cddl, ident) {
          if let Err(e) = chrono::DateTime::parse_from_rfc3339(s) {
            self.add_error(format!("expected tdate data type, decoding error: {}", e));
          }
        } else if is_ident_string_data_type(self.cddl, ident) {
          return Ok(());
        } else {
          self.add_error(format!("expected type {}, got {}", ident, self.json));
        }

        Ok(())
      }
      Value::Array(a) => {
        // Member keys are annotation only in an array context
        if self.is_member_key {
          return Ok(());
        }

        match validate_array_occurrence(
          self.occurrence.as_ref().take(),
          self.entry_counts.as_ref().map(|ec| &ec[..]),
          a,
        ) {
          Ok((iter_items, allow_empty_array)) => {
            if iter_items {
              for (idx, v) in a.iter().enumerate() {
                if let Some(indices) = &self.valid_array_items {
                  if self.is_multi_type_choice && indices.contains(&idx) {
                    continue;
                  }
                }

                let mut jv = JSONValidator::new(self.cddl, v.clone());
                jv.generic_rules = self.generic_rules.clone();
                jv.eval_generic_rule = self.eval_generic_rule;
                jv.is_multi_type_choice = self.is_multi_type_choice;
                jv.ctrl = self.ctrl.clone();
                jv.json_location
                  .push_str(&format!("{}/{}", self.json_location, idx));

                jv.visit_identifier(ident)?;

                if self.is_multi_type_choice && jv.errors.is_empty() {
                  if let Some(indices) = &mut self.valid_array_items {
                    indices.push(idx);
                  } else {
                    self.valid_array_items = Some(vec![idx]);
                  }
                  continue;
                }

                if let Some(errors) = &mut self.array_errors {
                  if let Some(error) = errors.get_mut(&idx) {
                    error.append(&mut jv.errors);
                  } else {
                    errors.insert(idx, jv.errors);
                  }
                } else {
                  let mut errors = HashMap::new();
                  errors.insert(idx, jv.errors);
                  self.array_errors = Some(errors)
                }
              }
            } else if let Some(idx) = self.group_entry_idx.take() {
              if let Some(v) = a.get(idx) {
                let mut jv = JSONValidator::new(self.cddl, v.clone());
                jv.generic_rules = self.generic_rules.clone();
                jv.eval_generic_rule = self.eval_generic_rule;
                jv.ctrl = self.ctrl.clone();
                jv.is_multi_type_choice = self.is_multi_type_choice;
                jv.json_location
                  .push_str(&format!("{}/{}", self.json_location, idx));

                jv.visit_identifier(ident)?;

                self.errors.append(&mut jv.errors);
              } else if !allow_empty_array {
                self.add_error(format!("expected type {} at index {}", ident, idx));
              }
            } else {
              self.add_error(format!("expected type {}, got {}", ident, self.json));
            }
          }
          Err(errors) => {
            for e in errors.into_iter() {
              self.add_error(e);
            }
          }
        }

        Ok(())
      }
      Value::Object(o) => {
        if let Some(occur) = &self.occurrence {
          if let Occur::ZeroOrMore(_) | Occur::OneOrMore(_) = occur {
            if let Occur::OneOrMore(_) = occur {
              if o.is_empty() {
                self.add_error(format!(
                  "object cannot be empty, one or more entries with key type {} required",
                  ident
                ));
                return Ok(());
              }
            }

            if is_ident_string_data_type(self.cddl, ident) {
              let mut errors = Vec::new();
              let values_to_validate = o
                .iter()
                .filter_map(|(k, v)| {
                  if let Some(keys) = &self.validated_keys {
                    if !keys.contains(&k) {
                      Some(v.clone())
                    } else {
                      None
                    }
                  } else {
                    errors.push(format!("key of type {} required, got {:?}", ident, k));
                    None
                  }
                })
                .collect::<Vec<_>>();

              self.values_to_validate = Some(values_to_validate);
              for e in errors.into_iter() {
                self.add_error(e);
              }

              return Ok(());
            }
          }
        }

        self.visit_value(&token::Value::TEXT(ident.ident))
      }
      _ => {
        if let Some(cut_value) = self.cut_value.take() {
          self.add_error(format!(
            "cut present for member key {}. expected type {}, got {}",
            cut_value, ident, self.json
          ));
        } else {
          self.add_error(format!("expected type {}, got {}", ident, self.json));
        }
        Ok(())
      }
    }
  }

  fn visit_value_member_key_entry(
    &mut self,
    entry: &ValueMemberKeyEntry<'a>,
  ) -> visitor::Result<ValidationError> {
    if let Some(occur) = &entry.occur {
      self.visit_occurrence(occur)?;
    }

    let current_location = self.json_location.clone();

    if let Some(mk) = &entry.member_key {
      let error_count = self.errors.len();
      self.is_member_key = true;
      self.visit_memberkey(mk)?;
      self.is_member_key = false;

      // Move to next entry if member key validation fails
      if self.errors.len() != error_count {
        self.advance_to_next_entry = true;
        return Ok(());
      }
    }

    if let Some(values) = &self.values_to_validate {
      for v in values.iter() {
        let mut jv = JSONValidator::new(self.cddl, v.clone());
        jv.generic_rules = self.generic_rules.clone();
        jv.eval_generic_rule = self.eval_generic_rule;
        jv.is_multi_type_choice = self.is_multi_type_choice;
        jv.is_multi_group_choice = self.is_multi_group_choice;
        jv.json_location.push_str(&self.json_location);
        jv.type_group_name_entry = self.type_group_name_entry;
        jv.visit_type(&entry.entry_type)?;

        self.json_location = current_location.clone();

        self.errors.append(&mut jv.errors);
        if entry.occur.is_some() {
          self.occurrence = None;
        }
      }

      return Ok(());
    }

    if let Some(v) = self.object_value.take() {
      let mut jv = JSONValidator::new(self.cddl, v);
      jv.generic_rules = self.generic_rules.clone();
      jv.eval_generic_rule = self.eval_generic_rule;
      jv.is_multi_type_choice = self.is_multi_type_choice;
      jv.is_multi_group_choice = self.is_multi_group_choice;
      jv.json_location.push_str(&self.json_location);
      jv.type_group_name_entry = self.type_group_name_entry;
      jv.visit_type(&entry.entry_type)?;

      self.json_location = current_location;

      self.errors.append(&mut jv.errors);
      if entry.occur.is_some() {
        self.occurrence = None;
      }

      Ok(())
    } else if !self.advance_to_next_entry {
      self.visit_type(&entry.entry_type)
    } else {
      Ok(())
    }
  }

  fn visit_type_groupname_entry(
    &mut self,
    entry: &TypeGroupnameEntry<'a>,
  ) -> visitor::Result<ValidationError> {
    self.type_group_name_entry = Some(entry.name.ident);
    walk_type_groupname_entry(self, entry)?;
    self.type_group_name_entry = None;

    Ok(())
  }

  fn visit_memberkey(&mut self, mk: &MemberKey<'a>) -> visitor::Result<ValidationError> {
    if let MemberKey::Type1 { is_cut, .. } = mk {
      self.is_cut_present = *is_cut;
    }

    walk_memberkey(self, mk)
  }

  fn visit_value(&mut self, value: &token::Value<'a>) -> visitor::Result<ValidationError> {
    let error: Option<String> = match &self.json {
      Value::Number(n) => match value {
        token::Value::INT(v) => match n.as_i64() {
          Some(i) => match &self.ctrl {
            Some(Token::NE) if i != *v as i64 => None,
            Some(Token::LT) if i < *v as i64 => None,
            Some(Token::LE) if i <= *v as i64 => None,
            Some(Token::GT) if i > *v as i64 => None,
            Some(Token::GE) if i >= *v as i64 => None,
            None => {
              if i == *v as i64 {
                None
              } else {
                Some(format!("expected value {}, got {}", v, n))
              }
            }
            _ => Some(format!(
              "expected value {} {}, got {}",
              self.ctrl.clone().unwrap(),
              v,
              n
            )),
          },
          None => Some(format!("{} cannot be represented as an i64", n)),
        },
        token::Value::UINT(v) => match n.as_u64() {
          Some(i) => match &self.ctrl {
            Some(Token::NE) if i != *v as u64 => None,
            Some(Token::LT) if i < *v as u64 => None,
            Some(Token::LE) if i <= *v as u64 => None,
            Some(Token::GT) if i > *v as u64 => None,
            Some(Token::GE) if i >= *v as u64 => None,
            Some(Token::SIZE) if i < 256u64.pow(*v as u32) => None,
            None => {
              if i == *v as u64 {
                None
              } else {
                Some(format!("expected value {}, got {}", v, n))
              }
            }
            _ => Some(format!(
              "expected value {} {}, got {}",
              self.ctrl.clone().unwrap(),
              v,
              n
            )),
          },
          None => Some(format!("{} cannot be represented as a u64", n)),
        },
        token::Value::FLOAT(v) => match n.as_f64() {
          Some(f) => match &self.ctrl {
            Some(Token::NE) if (f - *v).abs() > std::f64::EPSILON => None,
            Some(Token::LT) if f < *v as f64 => None,
            Some(Token::LE) if f <= *v as f64 => None,
            Some(Token::GT) if f > *v as f64 => None,
            Some(Token::GE) if f >= *v as f64 => None,
            None => {
              if (f - *v).abs() < std::f64::EPSILON {
                None
              } else {
                Some(format!("expected value {}, got {}", v, n))
              }
            }
            _ => Some(format!(
              "expected value {} {}, got {}",
              self.ctrl.clone().unwrap(),
              v,
              n
            )),
          },
          None => Some(format!("{} cannot be represented as an i64", n)),
        },
        _ => Some(format!("expected {}, got {}", value, n)),
      },
      Value::String(s) => match value {
        token::Value::TEXT(t) => match &self.ctrl {
          Some(Token::NE) => {
            if s != t {
              None
            } else {
              Some(format!("expected {} .ne to \"{}\"", value, s))
            }
          }
          Some(Token::REGEXP) | Some(Token::PCRE) => {
            let re = regex::Regex::new(
              &format_regex(
                serde_json::from_str::<Value>(&format!("\"{}\"", t))
                  .map_err(|e| ValidationError::from_validator(self, e.to_string()))?
                  .as_str()
                  .ok_or_else(|| {
                    ValidationError::from_validator(self, "malformed regex".to_string())
                  })?,
              )
              .ok_or_else(|| {
                ValidationError::from_validator(self, "malformed regex".to_string())
              })?,
            )
            .map_err(|e| ValidationError::from_validator(self, e.to_string()))?;

            if re.is_match(s) {
              None
            } else {
              Some(format!("expected \"{}\" to match regex \"{}\"", s, t))
            }
          }
          _ => {
            if s == t {
              None
            } else if let Some(ctrl) = &self.ctrl {
              Some(format!("expected value {} {}, got \"{}\"", ctrl, value, s))
            } else {
              Some(format!("expected value {} got \"{}\"", value, s))
            }
          }
        },
        token::Value::UINT(u) => match &self.ctrl {
          Some(Token::SIZE) => {
            if s.len() == *u {
              None
            } else {
              Some(format!("expected \"{}\" .size {}, got {}", s, u, s.len()))
            }
          }
          _ => Some(format!("expected {}, got {}", u, s)),
        },
        token::Value::BYTE(token::ByteValue::UTF8(b)) if s.as_bytes() == b.as_ref() => None,
        token::Value::BYTE(token::ByteValue::B16(b)) if s.as_bytes() == b.as_ref() => None,
        token::Value::BYTE(token::ByteValue::B64(b)) if s.as_bytes() == b.as_ref() => None,
        _ => Some(format!("expected {}, got \"{}\"", value, s)),
      },
      Value::Array(a) => {
        // Member keys are annotation only in an array context
        if self.is_member_key {
          return Ok(());
        }

        match validate_array_occurrence(
          self.occurrence.as_ref().take(),
          self.entry_counts.as_ref().map(|ec| &ec[..]),
          a,
        ) {
          Ok((iter_items, allow_empty_array)) => {
            if iter_items {
              for (idx, v) in a.iter().enumerate() {
                if let Some(indices) = &self.valid_array_items {
                  if self.is_multi_type_choice && indices.contains(&idx) {
                    continue;
                  }
                }

                let mut jv = JSONValidator::new(self.cddl, v.clone());
                jv.generic_rules = self.generic_rules.clone();
                jv.eval_generic_rule = self.eval_generic_rule;
                jv.is_multi_type_choice = self.is_multi_type_choice;
                jv.ctrl = self.ctrl.clone();
                jv.json_location
                  .push_str(&format!("{}/{}", self.json_location, idx));

                jv.visit_value(value)?;

                if self.is_multi_type_choice && jv.errors.is_empty() {
                  if let Some(indices) = &mut self.valid_array_items {
                    indices.push(idx);
                  } else {
                    self.valid_array_items = Some(vec![idx]);
                  }
                  continue;
                }

                if let Some(errors) = &mut self.array_errors {
                  if let Some(error) = errors.get_mut(&idx) {
                    error.append(&mut jv.errors);
                  } else {
                    errors.insert(idx, jv.errors);
                  }
                } else {
                  let mut errors = HashMap::new();
                  errors.insert(idx, jv.errors);
                  self.array_errors = Some(errors)
                }
              }
            } else if let Some(idx) = self.group_entry_idx.take() {
              if let Some(v) = a.get(idx) {
                let mut jv = JSONValidator::new(self.cddl, v.clone());
                jv.generic_rules = self.generic_rules.clone();
                jv.eval_generic_rule = self.eval_generic_rule;
                jv.is_multi_type_choice = self.is_multi_type_choice;
                jv.ctrl = self.ctrl.clone();
                jv.json_location
                  .push_str(&format!("{}/{}", self.json_location, idx));

                jv.visit_value(value)?;

                self.errors.append(&mut jv.errors);
              } else if !allow_empty_array {
                self.add_error(format!("expected value {} at index {}", value, idx));
              }
            } else {
              self.add_error(format!("expected value {}, got {}", value, self.json));
            }
          }
          Err(errors) => {
            for e in errors.into_iter() {
              self.add_error(e);
            }
          }
        }

        None
      }
      Value::Object(o) => {
        // Bareword member keys are converted to text string values
        if let token::Value::TEXT(t) = value {
          if self.is_cut_present {
            self.cut_value = Some(t);
          }

          if *t == "any" {
            return Ok(());
          }

          // Retrieve the value from key unless optional/zero or more, in which
          // case advance to next group entry
          if let Some(v) = o.get(*t) {
            self
              .validated_keys
              .get_or_insert(vec![t.to_string()])
              .push(t.to_string());
            self.object_value = Some(v.clone());
            self.json_location.push_str(&format!("/{}", t));

            None
          } else if let Some(Occur::Optional(_)) | Some(Occur::ZeroOrMore(_)) =
            &self.occurrence.take()
          {
            self.advance_to_next_entry = true;
            None
          } else if let Some(Token::NE) = &self.ctrl {
            None
          } else {
            Some(format!("object missing key: \"{}\"", t))
          }
        } else {
          Some(format!(
            "CDDL member key must be string data type. got {}",
            value
          ))
        }
      }
      _ => Some(format!("expected {}, got {}", value, self.json)),
    };

    if let Some(e) = error {
      self.add_error(e);
    }

    Ok(())
  }

  fn visit_occurrence(&mut self, o: &Occurrence) -> visitor::Result<ValidationError> {
    self.occurrence = Some(o.occur.clone());

    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn validate() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let cddl = r#"serivceEndpoint = ~uri
    "#;
    let json = r#""test""#;

    let mut lexer = lexer_from_str(cddl);
    let cddl = cddl_from_str(&mut lexer, cddl, true).map_err(json::Error::CDDLParsing)?;
    let json = serde_json::from_str::<serde_json::Value>(json).map_err(json::Error::JSONParsing)?;

    let mut jv = JSONValidator::new(&cddl, json);
    jv.validate()?;

    Ok(())
  }
}
