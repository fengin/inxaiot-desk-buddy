include!("resource_lease_repository.rs");

impl PartialOrd for LeaseRequest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LeaseRequest {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (
            &self.resource_type,
            &self.resource_key,
            &self.domain_type,
            &self.operation_id,
            &self.owner_instance_id,
            &self.owner_user,
            self.ttl,
        )
            .cmp(&(
                &other.resource_type,
                &other.resource_key,
                &other.domain_type,
                &other.operation_id,
                &other.owner_instance_id,
                &other.owner_user,
                other.ttl,
            ))
    }
}

