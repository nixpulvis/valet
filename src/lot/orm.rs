use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "lots")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub uuid: String,
    pub store: Vec<u8>,
    #[sea_orm(has_many, relation_enum = "Records")]
    pub records: HasMany<crate::record::orm::Entity>,
    #[sea_orm(has_many, relation_enum = "UserLot")]
    pub user_lots: HasMany<user_lots::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

pub mod user_lots {
    use sea_orm::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "user_lots")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub username: String,
        #[sea_orm(primary_key, column_name = "lot", auto_increment = false)]
        pub lot_uuid: String,
        pub name: String,
        pub data: Vec<u8>,
        pub nonce: Vec<u8>,
        #[sea_orm(belongs_to, relation_enum = "User", from = "username", to = "username")]
        pub user: HasOne<crate::user::orm::Entity>,
        #[sea_orm(belongs_to, relation_enum = "Lot", from = "lot_uuid", to = "uuid")]
        pub lot: HasOne<super::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}
