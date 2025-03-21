use crate::Perform;
use actix_web::web::Data;
use lemmy_api_common::{
  community::{BanFromCommunity, BanFromCommunityResponse},
  context::LemmyContext,
  utils::{get_local_user_view_from_jwt, is_mod_or_admin, remove_user_data_in_community},
  websocket::UserOperation,
};
use lemmy_db_schema::{
  source::{
    community::{
      CommunityFollower,
      CommunityFollowerForm,
      CommunityPersonBan,
      CommunityPersonBanForm,
    },
    moderator::{ModBanFromCommunity, ModBanFromCommunityForm},
  },
  traits::{Bannable, Crud, Followable},
};
use lemmy_db_views_actor::structs::PersonViewSafe;
use lemmy_utils::{error::LemmyError, utils::time::naive_from_unix, ConnectionId};

#[async_trait::async_trait(?Send)]
impl Perform for BanFromCommunity {
  type Response = BanFromCommunityResponse;

  #[tracing::instrument(skip(context, websocket_id))]
  async fn perform(
    &self,
    context: &Data<LemmyContext>,
    websocket_id: Option<ConnectionId>,
  ) -> Result<BanFromCommunityResponse, LemmyError> {
    let data: &BanFromCommunity = self;
    let local_user_view =
      get_local_user_view_from_jwt(&data.auth, context.pool(), context.secret()).await?;

    let community_id = data.community_id;
    let banned_person_id = data.person_id;
    let remove_data = data.remove_data.unwrap_or(false);
    let expires = data.expires.map(naive_from_unix);

    // Verify that only mods or admins can ban
    is_mod_or_admin(context.pool(), local_user_view.person.id, community_id).await?;

    let community_user_ban_form = CommunityPersonBanForm {
      community_id: data.community_id,
      person_id: data.person_id,
      expires: Some(expires),
    };

    if data.ban {
      CommunityPersonBan::ban(context.pool(), &community_user_ban_form)
        .await
        .map_err(|e| LemmyError::from_error_message(e, "community_user_already_banned"))?;

      // Also unsubscribe them from the community, if they are subscribed
      let community_follower_form = CommunityFollowerForm {
        community_id: data.community_id,
        person_id: banned_person_id,
        pending: false,
      };

      CommunityFollower::unfollow(context.pool(), &community_follower_form)
        .await
        .ok();
    } else {
      CommunityPersonBan::unban(context.pool(), &community_user_ban_form)
        .await
        .map_err(|e| LemmyError::from_error_message(e, "community_user_already_banned"))?;
    }

    // Remove/Restore their data if that's desired
    if remove_data {
      remove_user_data_in_community(community_id, banned_person_id, context.pool()).await?;
    }

    // Mod tables
    let form = ModBanFromCommunityForm {
      mod_person_id: local_user_view.person.id,
      other_person_id: data.person_id,
      community_id: data.community_id,
      reason: data.reason.clone(),
      banned: Some(data.ban),
      expires,
    };

    ModBanFromCommunity::create(context.pool(), &form).await?;

    let person_id = data.person_id;
    let person_view = PersonViewSafe::read(context.pool(), person_id).await?;

    let res = BanFromCommunityResponse {
      person_view,
      banned: data.ban,
    };

    context
      .chat_server()
      .send_community_room_message(
        &UserOperation::BanFromCommunity,
        &res,
        community_id,
        websocket_id,
      )
      .await?;

    Ok(res)
  }
}
