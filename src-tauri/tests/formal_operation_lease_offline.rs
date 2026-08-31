use std::collections::BTreeSet;
use std::time::Duration;

use inxaiot_desk_buddy_lib::formal::resource_lease_repository::LeaseRequest;

#[test]
fn lease_requests_have_deterministic_resource_order() {
    let requests = ["B", "A", "C", "A"]
        .into_iter()
        .map(|key| LeaseRequest {
            resource_type: "aio".into(),
            resource_key: key.into(),
            domain_type: "aio".into(),
            operation_id: "operation-a".into(),
            owner_instance_id: "instance-a".into(),
            owner_user: "user-a".into(),
            ttl: Duration::from_secs(30),
        })
        .map(|request| {
            (
                (request.resource_type.clone(), request.resource_key.clone()),
                request,
            )
        })
        .collect::<BTreeSet<_>>();
    let keys = requests
        .into_iter()
        .map(|((_, key), _)| key)
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["A", "B", "C"]);
}
