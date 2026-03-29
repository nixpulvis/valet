use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub username: String,
    pub salt: Vec<u8>,
    pub validation_data: Vec<u8>,
    pub validation_nonce: Vec<u8>,

    #[sea_orm(has_many, relation_enum = "UserLot", via_rel = "User")]
    pub user_lots: HasMany<crate::lot::orm::user_lots::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

impl Related<crate::lot::orm::Entity> for Entity {
    fn to() -> RelationDef {
        crate::lot::orm::user_lots::Relation::Lot.def()
    }
    fn via() -> Option<RelationDef> {
        Some(Relation::UserLot.def())
    }
}

/// Traverses User -> UserLots -> Lots -> Records.
#[allow(dead_code)]
pub struct UserToRecords;

impl Linked for UserToRecords {
    type FromEntity = Entity;
    type ToEntity = crate::record::orm::Entity;

    fn link(&self) -> Vec<RelationDef> {
        vec![
            Relation::UserLot.def(),
            crate::lot::orm::user_lots::Relation::Lot.def(),
            crate::lot::orm::Relation::Records.def(),
        ]
    }
}
