use bimap::BiMap;
use web_sys::{WebSocket, MessageEvent};
use crossbeam_channel::{Receiver, Sender};
use quoridor_core::{rulebooks::*, *};
use std::{cell::RefCell, error::Error, rc::Rc};
use tbmp_core::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[allow(unused_macros)]
macro_rules! console_log {
    ($($t:tt)*) => ( #[allow(unused_unsafe)]unsafe { log(&format_args!($($t)*).to_string()) })
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}


generate_rulebook! {
    [NO CONNECT]
    FreeQuoridor,
    StandardQuoridor,
}

const STANDARD_CANVAS_SIZE: f64 = 150.0;
const WALL_TO_SPOT_RATIO: f64 = 2.5;
const WALL_WIDTH: f64 = STANDARD_CANVAS_SIZE / (10.0 + WALL_TO_SPOT_RATIO * 9.0);
const SPOT_WIDTH: f64 = WALL_WIDTH * WALL_TO_SPOT_RATIO;
const UNIT_WIDTH: f64 = WALL_WIDTH + SPOT_WIDTH;

thread_local! {
    static COLORS: RefCell<ColorStruct> = RefCell::new(
        ColorStruct {
            base: JsValue::from_str("#50190A"),
            wall_slot: JsValue::from_str("#743c0d"),
            wall: JsValue::from_str("#996F38"),
            select: JsValue::from_str("#ACACAC"),
            pawns: vec![]
        }
    );
}

fn get_colors() -> ColorStruct {
    COLORS.with(|colors| colors.borrow().clone())
}

fn set_colors(new_colors: ColorStruct) {
    COLORS.with(|colors| *colors.borrow_mut() = new_colors);
}

/*fn get_context() -> Option<web_sys::CanvasRenderingContext2d> {
    web_sys::window()?
        .document()?
        .get_element_by_id("canvas")?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()?
        .get_context("2d")
        .ok()??
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .ok()
}*/

#[derive(Debug)]
struct State {
    highlight: Option<Position>,
}

#[derive(Clone)]
struct ColorStruct {
    base: JsValue,
    wall_slot: JsValue,
    wall: JsValue,
    select: JsValue,
    pawns: Vec<JsValue>,
}

#[wasm_bindgen(start)]
pub fn start() {
    main().unwrap();
}

fn main() -> Option<()> {
    let document = web_sys::window()?.document()?; 

    let canvas = document.create_element("canvas").unwrap();
    let main_div = document.get_element_by_id("divvv").unwrap();

    main_div.append_child(&canvas).unwrap();

    let width = web_sys::window()?.inner_width().ok()?.as_f64()? as u32;
    let height = web_sys::window()?.inner_height().ok()?.as_f64()? as u32;

    let size = u32::min(width, height);

    let canvas: web_sys::HtmlCanvasElement =
        canvas.dyn_into::<web_sys::HtmlCanvasElement>().ok()?;

    canvas.set_width(size);
    canvas.set_height(size);

    if width > height {
        let margin = (width - size) / 2;
        canvas
            .style()
            .set_property("margin-left", &format!("{}px", margin))
            .ok()?;
        canvas
            .style()
            .set_property("margin-right", &format!("{}px", margin))
            .ok()?;
    } else {
        let margin = (height - size) / 2;
        canvas
            .style()
            .set_property("margin-top", &format!("{}px", margin))
            .ok()?;
        canvas
            .style()
            .set_property("margin-bottom", &format!("{}px", margin))
            .ok()?;
    }

    let location: String = web_sys::window()?.location().href().ok()?;
    let game_name = location.split('/').rev().find(|s| !s.is_empty()).unwrap();
    
    let mut ws = WebSocket::new(&format!("ws://localhost:3030/join/{}", game_name)).ok()?;
    
    let context = canvas
    .get_context("2d")
    .ok()??
    .dyn_into::<web_sys::CanvasRenderingContext2d>()
    .ok()?;
    
    let scale = size as f64 / STANDARD_CANVAS_SIZE;
    context.scale(scale, scale).ok()?;
    
    let agent = QAgent::FreeQuoridor(WSAgent::<QGame::<FreeQuoridor>>::connect(&mut ws));

    let ocnt = Closure::once(move || {
        console_log!("connection ready!");
    });

    fn rec(agent: QAgent, context: web_sys::CanvasRenderingContext2d, size: u32, canvas: web_sys::HtmlCanvasElement) {
        console_log!("lööps");
        if let Ok(msg) = agent.recv_event() {
            let (game, side) = match msg {
                QGameEvent::GameStart(g, s) => (g, s),
                _ => unreachable!()
            };
            on_connect(agent, game, side, context, size, canvas)
        } else {
            //rec(agent, context, side, size, canvas);
            let r = Closure::once(move || {
                rec(agent, context, size, canvas);
            });
            web_sys::window().unwrap().set_timeout_with_callback_and_timeout_and_arguments_0(r.as_ref().unchecked_ref(), 100).unwrap();
            r.forget();
        }
    }

    rec(agent, context, size, canvas);

    ws.set_onopen(Some(ocnt.as_ref().unchecked_ref()));
    ocnt.forget();
    Some(())
}

fn on_connect(agent: QAgent, game: Quoridor, side: PlayerID, context: web_sys::CanvasRenderingContext2d, size: u32, canvas: web_sys::HtmlCanvasElement) {
    let mut colors = get_colors();
    for i in 0..game.get_pawn_count() {
        let color = format!(
            "hsl({},100%,50%)",
            i as f64 * 360.0 / game.get_pawn_count() as f64
        );
        colors.pawns.push(JsValue::from_str(&color));
    }
    set_colors(colors);

    let state = State { highlight: None };
    render_game(&context, &game, &state);

    let rc = Rc::new((
        RefCell::new(game),
        RefCell::new(context),
        RefCell::new(state),
        RefCell::new(side),
        RefCell::new(agent),
    ));
    
    let rcc = Clone::clone(&rc);
    let click_handler = move |event: web_sys::MouseEvent| {
        let game = rcc.0.borrow_mut();
        let context = rcc.1.borrow_mut();
        let mut state = rcc.2.borrow_mut();
        let side = rcc.3.borrow();
        let agent = rcc.4.borrow_mut();

        let x = STANDARD_CANVAS_SIZE * event.offset_x() as f64 / size as f64;
        let y = STANDARD_CANVAS_SIZE * event.offset_y() as f64 / size as f64;

        //console::log_2(&x.into(), &x.into());

        let mod_x = x % UNIT_WIDTH;
        let mod_y = y % UNIT_WIDTH;

        let x = ((x - mod_x) / UNIT_WIDTH) as u8;
        let y = ((y - mod_y) / UNIT_WIDTH) as u8;

        match (mod_x > WALL_WIDTH, mod_y > WALL_WIDTH) {
            (true, true) => {
                let pos = Position::from((x, 8 - y));
                state.highlight = match (game.pawns().get_by_right(&pos), state.highlight) {
                    (Some(id), _) if id.owned_by(&game) != *side => None,
                    (Some(_), None) => Some(pos),
                    (Some(_), Some(hpos)) if hpos != pos => Some(pos),
                    (None, Some(hpos)) => { 
                        if game.turn_of() == *side {
                            //send a move
                            let qmv = Move::MovePawn(hpos, pos); 
                            agent.send_move(RulebookMove::wrap(&game, &qmv)).unwrap();
                        }
                        None
                    },
                    _ => None
                };
            }
            (false, false) => {
                let wall = Wall {
                    position: (x, 9 - y).into(),
                    orientation: if event.button() == 0 {
                        Orientation::Vertical 
                    } else {
                        Orientation::Horizontal 
                    },
                    wall_type: WallType::Simple,
                };

                if game.turn_of() == *side {
                    //send a move
                    let qmv = Move::PlaceWall(wall);
                    agent.send_move(RulebookMove::wrap(&game, &qmv)).unwrap();
                }
            }
            (horizontal, _vertical) => {
                let _ = Wall {
                    position: (x, horizontal as u8 + 8 - y).into(),
                    orientation: if horizontal {
                        Orientation::Horizontal
                    } else {
                        Orientation::Vertical
                    },
                    wall_type: WallType::Single,
                };

                if game.turn_of() == *side {
                    //send a move
                }
            }
        }

        //console::log_1(&strr.into());

        render_game(&context, &game, &state);
    };

    let rcc = Clone::clone(&rc);
    let game_event_handler = move || {
        let mut game = rcc.0.borrow_mut();
        let context = rcc.1.borrow_mut();
        let state = rcc.2.borrow();
        //let side = rcc.3.borrow();
        let agent = rcc.4.borrow();

        if let Ok(e) = agent.recv_event() {
            match e {
                QGameEvent::MoveHappened(qmv) => { game.apply_move(&qmv); }
                QGameEvent::GameEnd(pid) =>{
                    if let Some(id) = pid { 
                        console_log!("PLayer {} won!", id);
                    } else { 
                        console_log!("DRAW");
                    }
                },
                _ => {}
            }
            render_game(&context, &game, &state);
        }
    };

    let closure = Closure::wrap(Box::new(click_handler) as Box<dyn FnMut(web_sys::MouseEvent)>);
    canvas.set_onclick(Some(closure.as_ref().unchecked_ref()));
    closure.forget();

    let closure = Closure::wrap(Box::new(game_event_handler) as Box<dyn FnMut()>);
    let window = web_sys::window().unwrap();
    window.set_interval_with_callback_and_timeout_and_arguments_0(
        closure.as_ref().unchecked_ref(),
        100,
    ).unwrap();
    closure.forget();
}

fn render_game(context: &web_sys::CanvasRenderingContext2d, game: &Quoridor, state: &State) {
    context.set_fill_style(&get_colors().base);
    context.fill_rect(0.0, 0.0, STANDARD_CANVAS_SIZE, STANDARD_CANVAS_SIZE);

    let colors = &get_colors();

    context.set_fill_style(&colors.wall_slot);
    for i in 0..10 {
        context.fill_rect(
            i as f64 * (WALL_WIDTH * (1.0 + WALL_TO_SPOT_RATIO)),
            0.0,
            WALL_WIDTH,
            STANDARD_CANVAS_SIZE,
        );
        context.fill_rect(
            0.0,
            i as f64 * (WALL_WIDTH * (1.0 + WALL_TO_SPOT_RATIO)),
            STANDARD_CANVAS_SIZE,
            WALL_WIDTH,
        );
    }

    context.set_fill_style(&colors.wall);
    for wall in game.walls().iter() {
        let h_control = (wall.orientation == Orientation::Horizontal) as u8 as f64;
        let v_conrol = (wall.orientation == Orientation::Vertical) as u8 as f64;

        match wall.wall_type {
            WallType::Simple => {
                let x = wall.position.x as f64 * UNIT_WIDTH - h_control * SPOT_WIDTH;
                let y = (9 - wall.position.y) as f64 * UNIT_WIDTH - v_conrol * SPOT_WIDTH;
                context.fill_rect(
                    x,
                    y,
                    WALL_WIDTH * v_conrol + (UNIT_WIDTH + SPOT_WIDTH) * h_control,
                    WALL_WIDTH * h_control + (UNIT_WIDTH + SPOT_WIDTH) * v_conrol,
                );
            }
            WallType::Single => unimplemented!("Can't render single walls! ( Yet ;) )"),
            WallType::Strong => unimplemented!("Can't strong single walls! ( Yet ;) )"),
        }
    }

    for (&id, &pos) in game.pawns().iter() {
        let (x, y) = (pos.x as f64, (8 - pos.y) as f64);

        let color = match state.highlight {
            Some(hpos) if hpos == pos => &colors.select,
            _ => &colors.pawns[id as usize],
        };

        context.set_fill_style(color);

        context.fill_rect(
            WALL_WIDTH + x * UNIT_WIDTH,
            WALL_WIDTH + y * UNIT_WIDTH,
            SPOT_WIDTH,
            SPOT_WIDTH,
        );
    }
}

trait PID {
    fn owned_by(&self, game: &Quoridor) -> u8;
}

impl PID for PawnID {
    fn owned_by(&self, game: &Quoridor) -> u8 {
        let p3 = game.get_pawn_count() / game.get_player_count();

        self / p3
    }
}

trait WSAgent<G: Game> {
    fn connect(&mut self) -> AgentCore<G>; 
}

impl<G: Game> WSAgent<G> for WebSocket {
    fn connect(&mut self) -> AgentCore<G> {
        console_log!("connectin");
        let ws = self.clone();
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let (etx, erx) = crossbeam_channel::unbounded();
        let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
            console_log!("RECEIVIN SHIT");
            web_sys::console::log_1(&e.data());
            if let Ok(abuf) = e.data().dyn_into::<js_sys::ArrayBuffer>() {
                console_log!("deserializin the shit");
                let array = js_sys::Uint8Array::new(&abuf);
                let event = bincode::deserialize(&array.to_vec()).unwrap();
                console_log!("sent the shit");
                etx.send(event).unwrap();
            }
        }) as Box<dyn FnMut(MessageEvent)>);

        let ws = self.clone();
        ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
        onmessage_callback.forget();

        let (mtx, mrx) = crossbeam_channel::unbounded();
        
        let cb = Closure::wrap(Box::new(move || {
            if let Ok(qmove) = mrx.try_recv() {
                let buf = bincode::serialize(&qmove).unwrap();
                ws.send_with_u8_array(&buf).unwrap();
            }
        }) as Box<dyn FnMut()>);
    
        let window = web_sys::window().unwrap();
        window.set_interval_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            100,
        ).unwrap();
        cb.forget();

        AgentCore {
            event_channel: erx,
            move_channel: mtx,
        }
    }
}
