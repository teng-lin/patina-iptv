//! Portable identity and accounting for shared live inputs. Hosts own transport,
//! credentials and stream lifetimes; an identity never grants access by itself.

/// Numeric playlist IDs may be reused. Only the same durable incarnation and
/// channel can share an incoming stream; transport/auth checks remain required.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub source: i64,
    pub incarnation: String,
    pub channel: String,
}

/// One connection per independent consumer; consumers retaining the same actual
/// input count once. IDs are process-local host capabilities, never source IDs.
pub fn upstream_count(inputs: impl IntoIterator<Item = Option<u64>>) -> usize {
    let mut shared = Vec::new();
    let mut independent = 0;
    for input in inputs {
        match input {
            Some(id) if !shared.contains(&id) => shared.push(id),
            Some(_) => {}
            None => independent += 1,
        }
    }
    independent + shared.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_actual_shared_inputs_reduce_connection_cost() {
        assert_eq!(upstream_count([None, None]), 2);
        assert_eq!(upstream_count([Some(1), Some(1)]), 1);
        assert_eq!(upstream_count([Some(1), Some(2), Some(1), None]), 3);
        let old = Identity {
            source: 1,
            incarnation: "generated-old".into(),
            channel: "generated-a".into(),
        };
        assert_ne!(
            old,
            Identity {
                incarnation: "generated-new".into(),
                ..old.clone()
            }
        );
        assert_ne!(
            old,
            Identity {
                channel: "generated-b".into(),
                ..old.clone()
            }
        );
    }
}
