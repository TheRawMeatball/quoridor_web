// #![deny(warnings)]
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use warp::{hyper::Uri, Filter};
use warp::{
    path,
    ws::{Message, WebSocket},
    Rejection,
};

use bimap::BiMap;
use crossbeam_channel::{Receiver, Sender};
use quoridor_core::{rulebooks::*, *};
use std::error::Error;
use tbmp::*;

generate_rulebook! {
    StandardQuoridor,
    FreeQuoridor,
}

type GameFn = Box<dyn Send + Sync + FnMut() -> Result<MoveResult, Box<dyn Error>>>;
type Lobbies = Arc<RwLock<HashMap<String, (Vec<QAgent>, QGameType, GameFn)>>>;
type Games = Arc<RwLock<HashMap<String, GameFn>>>;

#[derive(Serialize, Deserialize)]
struct LobbyRequest {
    game_type: String,
    name: String,
}

macro_rules! warpify {
    ($x:ident) => {{
        let c = $x.clone();
        warp::any().map(move || c.clone())
    }};
}

fn gtstr(gt: &QGameType) -> &'static str {
    match gt {
        QGameType::StandardQuoridor => "standard",
        QGameType::FreeQuoridor => "free",
    }
}

async fn get_lobbies(lobbies: Lobbies) -> Result<impl warp::Reply, Infallible> {
    Ok(warp::reply::json(
        &lobbies
            .read()
            .await
            .iter()
            .map(|(name, (_, game_type, _))| LobbyRequest {
                game_type: gtstr(game_type).into(),
                name: name.clone(),
            })
            .collect::<Vec<_>>(),
    ))
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let games = Games::default();

    let lobbies = Lobbies::default();

    let new_lobby = warp::post()
        .and(path!("lobby" / "new"))
        .and(parse_lobby_request())
        .and(warpify!(lobbies))
        .and_then(
            |(game_type, name): (QGameType, String), lobbies: Lobbies| async move {
                let (v, t) = game_type.new_game();
                let gt = game_type;
                let n = name.clone();
                lobbies.write().await.insert(name, (v, game_type, t));
                Ok::<_, std::convert::Infallible>(warp::redirect(
                    Uri::builder()
                        .path_and_query(&format!("/game/{}/{}", gtstr(&gt), n)[..])
                        .build()
                        .unwrap(),
                ))
            },
        );

    let lobby_list = warp::get()
        .and(path!("lobby" / "list"))
        .and(warpify!(lobbies))
        .and_then(get_lobbies);

    let join = warp::get()
        .and(path!("join" / String))
        .and(warpify!(lobbies))
        .and(warpify!(games))
        .and(warp::ws())
        .map(
            |name: String, lobbies: Lobbies, games: Games, socket: warp::ws::Ws| {
                socket.on_upgrade(|socket| async move {
                    let arc = Clone::clone(&lobbies);
                    let mut lobbies = lobbies.write().await;
                    let agent = lobbies.get_mut(&name).unwrap().0.pop().unwrap();
                    if lobbies.get(&name).unwrap().0.len() == 0 {
                        let game = lobbies.remove(&name).unwrap();
                        drop(lobbies);
                        games.write().await.insert(name.clone(), game.2);
                    } else {
                        drop(lobbies);
                    }
                    match agent {
                        QAgent::StandardQuoridor(c) => c.host(socket, games, arc, name),
                        QAgent::FreeQuoridor(c) => c.host(socket, games, arc, name),
                    }
                })
            },
        );

    //let game = warp::path::end().map(|| warp::reply::html(GAME_HTML));
    let game = path!("game" / String / String)
        .and(warp::fs::file("./static/game.html"))
        .map(|_, _, f: warp::fs::File| f);
    //let index = warp::path::end().map(|| warp::reply::html(INDEX_HTML));
    let index = warp::path::end()
        .and(warp::fs::file("./static/index.html"))
        .map(|f: warp::fs::File| f);

    //println!("{:?}", std::fs::canonicalize(std::path::PathBuf::from("./static")));

    let routes = index
        .or(game)
        .or(lobby_list)
        .or(new_lobby)
        .or(join)
        .or(path("static").and(
            warp::fs::dir("./static")
                .map(|f: warp::fs::File| warp::reply::with_header(f, "name", "value")),
        ));

    warp::serve(routes).run(([0, 0, 0, 0], 3030)).await;
}

fn parse_lobby_request() -> impl Filter<Extract = ((QGameType, String),), Error = Rejection> + Copy
{
    warp::body::form().and_then(|gt: LobbyRequest| async move {
        let game_type = match &gt.game_type[..] {
            "standard" => QGameType::StandardQuoridor,
            "free" => QGameType::FreeQuoridor,
            _ => return Err(warp::reject::custom(UnimplementedGameType)),
        };

        Ok((game_type, gt.name))
    })
}

trait WSHost {
    fn host(self, socket: WebSocket, games: Games, lobbies: Lobbies, name: String);
}

impl<G: Game> WSHost for AgentCore<G> {
    fn host(self, socket: WebSocket, games: Games, lobbies: Lobbies, name: String) {
        let (wstx, mut wsrx) = socket.split();

        let (tx, rx) = mpsc::unbounded_channel();
        //let quit_tx = tx.clone();
        tokio::spawn(rx.forward(wstx));
        let mc = self.move_channel;
        let ec = self.event_channel;
        tokio::spawn(async move {
            while let Some(result) = wsrx.next().await {
                match result {
                    Ok(msg) => {
                        let buf = msg.as_bytes();
                        if let Ok(qmv) = bincode::deserialize::<G::Move>(buf) {
                            mc.send(qmv).unwrap();
                            if let Some(t) = games.write().await.get_mut(&name) {
                                t().unwrap();
                            }
                        } else {
                            //let buf = bincode::serialize(&GameEvent::<G>::OpponentQuit).unwrap();
                            eprintln!("Someone quit!");
                            games.write().await.remove(&name);
                            lobbies.write().await.remove(&name);
                            //quit_tx.send(Ok(Message::binary(buf))).unwrap();
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        tokio::spawn(async move {
            loop {
                if let Ok(msg) = ec.try_recv() {
                    let buf = bincode::serialize(&msg).unwrap();
                    tx.send(Ok(Message::binary(buf))).unwrap();
                }
                tokio::task::yield_now().await;
            }
        });
    }
}

#[derive(Debug)]
struct UnimplementedGameType;
impl warp::reject::Reject for UnimplementedGameType {}
