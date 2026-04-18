use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "records")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub uuid: String,
    #[sea_orm(column_name = "lot")]
    pub lot_uuid: String,
    pub label: Vec<u8>,
    pub label_nonce: Vec<u8>,
    pub data: Vec<u8>,
    pub data_nonce: Vec<u8>,
    #[sea_orm(belongs_to, from = "lot_uuid", to = "uuid")]
    pub lot: HasOne<crate::lot::orm::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
