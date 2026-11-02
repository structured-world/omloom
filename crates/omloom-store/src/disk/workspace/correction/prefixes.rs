use super::*;

impl Tx<'_> {
    pub(super) fn prefix_correction_guard(
        &self,
        context: &Context,
        target: AssetRef,
    ) -> Result<()> {
        let row = self
            .get(&format!("network/{}", network::identity(target)))?
            .ok_or(Error::Missing)?;
        for field in ["cidr", "address_space", "link_scope", "routing_context"] {
            if row.contains_key(field)
                && !context.field(target, wire::AssetKind::Prefix as i32, field, false)
            {
                return Err(Error::Denied);
            }
        }
        if !context.field(target, wire::AssetKind::Prefix as i32, "network", true) {
            return Err(Error::Denied);
        }
        self.require_no_ipam_references(target)?;
        self.require_no_physical_reservations(target)?;
        if !self
            .children(&format!("members/{}", network::identity(target)))?
            .is_empty()
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }

    pub(super) fn merge_prefix_state(
        &mut self,
        context: &Context,
        from: AssetRef,
        to: AssetRef,
    ) -> Result<()> {
        self.prefix_correction_guard(context, from)?;
        self.prefix_correction_guard(context, to)?;
        let source_key = format!("network/{}", network::identity(from));
        let target_key = format!("network/{}", network::identity(to));
        let source = self.get(&source_key)?.ok_or(Error::Missing)?;
        let previous = self.get(&target_key)?.ok_or(Error::Missing)?;
        if source.get("cidr") != previous.get("cidr") {
            return Err(Error::Conflict);
        }
        let mut data = strip(previous.clone());
        for field in [
            "address_space",
            "link_scope",
            "routing_context",
            "namespace",
        ] {
            if let Some(value) = source.get(field) {
                if previous.get(field).is_some_and(|old| old != value) {
                    return Err(Error::Conflict);
                }
                data.insert(field.into(), value.clone());
            }
        }
        // The allocation keeps its exact domain key, changing only its explicit
        // reviewed owner. No address allocation or provider identity is merged.
        if let Some(Value::String(namespace)) = source.get("namespace") {
            let owner = self.get(namespace)?.ok_or(Error::Corrupt)?;
            if crate::disk::target(&owner)? != from {
                return Err(Error::Conflict);
            }
            self.put(namespace, "allocations", Class::Inventory, target_props(to))?;
        }
        self.check_prefix_members(context, to, &data)?;
        self.put_against(
            &target_key,
            "networks",
            Class::Inventory,
            data,
            Some(&previous),
        )?;
        self.delete(&source_key)?;
        Ok(())
    }
}
