// #![deny(warnings)]
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use futures::{FutureExt, StreamExt};
use tokio::sync::{mpsc, RwLock};
use warp::{ws::{Message, WebSocket}, path};
use warp::Filter;

use quoridor_core::{*, rulebooks::*};
use tbmp::*;
use bimap::BiMap;
use std::error::Error;

generate_rulebook! {
    StandardQuoridor,
    FreeQuoridor,
}

/// Our global unique user id counter.
static NEXT_USER_ID: AtomicUsize = AtomicUsize::new(1);
static NEXT_GAME_ID: AtomicUsize = AtomicUsize::new(1);

/// Our state of currently connected users.
///
/// - Key is their id
/// - Value is a sender of `warp::ws::Message`
type Users = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Result<Message, warp::Error>>>>>;

/// key is game id, value is a tuple containing the game thread, a vec with all unconnected agents and a vec with ids for all connected players
type Games = Arc<RwLock<HashMap<usize, (Box<dyn Send + Sync + FnMut() -> Result<MoveResult, Box<dyn Error>>>, Vec<QAgent>, Vec<usize>)>>>;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let users = Users::default();
    let users = warp::any().map(move || users.clone());

    let games = Games::default();
    let games = warp::any().map(move || games.clone());

    // GET /chat -> websocket upgrade
    let chat = path!("game" / String)
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(users)
        .and(games)
        .map(|game_type: String, ws: warp::ws::Ws, users, games| {
            // This will call our function if the handshake succeeds.
            let game_type = match &game_type[..] {
                "free" => Some(QGameType::FreeQuoridor),
                "standard" => Some(QGameType::StandardQuoridor),
                _ => None
            };

            ws.on_upgrade(move |socket| user_connected(socket, users, games, game_type))
        });

    // GET / -> index html
    let index = warp::path::end().map(|| warp::reply::html(INDEX_HTML));

    let routes = index.or(chat);

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}

async fn user_connected(ws: WebSocket, users: Users, games: Games, game_type: Option<QGameType>) {
    // Use a counter to assign a new unique ID for this user.
    let game_type = if let Some(gt) = game_type { gt } else { return; };
    
    let player_id = NEXT_USER_ID.fetch_add(1, Ordering::Relaxed);
    let game_id = NEXT_GAME_ID.fetch_add(1, Ordering::Relaxed);
    let (agents, thread) = game_type.new_game();
    
    eprintln!("new chat user: {}", player_id);

    // Split the socket into a sender and receive of messages.
    let (user_ws_tx, mut user_ws_rx) = ws.split();

    // Use an unbounded channel to handle buffering and flushing of messages
    // to the websocket...
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::task::spawn(rx.forward(user_ws_tx).map(|result| {
        if let Err(e) = result {
            eprintln!("websocket send error: {}", e);
        }
    }));

    // Save the sender in our list of connected users.
    users.write().await.insert(player_id, tx);
    games.write().await.insert(game_id, (thread, agents,vec![player_id]));

    // Make an extra clone to give to our disconnection handler...
    let users2 = users.clone();
    let games2 = games.clone();

    // Every time the user sends a message, broadcast it to
    // all other users...
    while let Some(result) = user_ws_rx.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("websocket error(uid={}): {}", player_id, e);
                break;
            }
        };
        user_message(player_id, game_id, msg, &users, &games).await;
    }

    // user_ws_rx stream will keep processing as long as the user stays
    // connected. Once they disconnect, then...
    user_disconnected(player_id, game_id, &users2, &games2).await;
}

async fn user_message(my_id: usize, game_id: usize, msg: Message, users: &Users, games: &Games) {
    let msg = msg.as_bytes();

    

    /*
    // Skip any non-Text messages...

    let new_msg = format!("<User#{}>: {}", my_id, msg);

    // New message from this user, send it to everyone else (except same uid)...
    for (&uid, tx) in users.read().await.iter() {
        if my_id != uid {
            if let Err(_disconnected) = tx.send(Ok(Message::text(new_msg.clone()))) {
                // The tx is disconnected, our `user_disconnected` code
                // should be happening in another task, nothing more to
                // do here.
            }
        }
    }*/

    games.write().await.get_mut(&game_id).unwrap().0().unwrap();
}

async fn user_disconnected(player_id: usize, game_id: usize, users: &Users, games: &Games) {
    eprintln!("good bye user: {}", player_id);

    // Stream closed up, so remove from the user list

    for player in &games.read().await.get(&game_id).unwrap().2 {
        let mut players = users.write().await;
        let player_sender = players.get(player).unwrap();
        if *player != player_id {
            player_sender.send(Ok(Message::text(""))).ok();
        }
        players.remove(&player);
    }

}

static INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
    <head>
        <title>Warp Chat</title>
    </head>
    <body>
        
    </body>
</html>
"#;