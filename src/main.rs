// this is free and unencumbered software released into the public domain.
// see the attached UNLICENSE or https://unlicense.org

#![deny(bare_trait_objects)]

extern crate servo;
extern crate glutin;

use std::env;
use std::rc::Rc;
use std::sync::Arc;
use std::path::PathBuf;
use std::mem;
use std::cell::Cell;

use glutin::{Event, WindowEvent, EventsLoop, EventsLoopProxy, TouchPhase,
  MouseScrollDelta, MouseButton, ElementState};
use glutin::dpi::{LogicalPosition, LogicalSize, PhysicalPosition,
  PhysicalSize};

use servo::embedder_traits::{EventLoopWaker, resources, EmbedderMsg};
use servo::embedder_traits::resources::{Resource, ResourceReaderMethods};
use servo::servo_config::opts;
use servo::{gl, Servo, BrowserId};
use servo::gl::GlFns;
use servo::servo_url::ServoUrl;
use servo::compositing::windowing::{WindowMethods, EmbedderCoordinates,
  AnimationState, WindowEvent as ServoWindowEvent,
  MouseWindowEvent as ServoMouseWindowEvent};
use servo::euclid::{TypedPoint2D, TypedRect, TypedScale, TypedSize2D,
  TypedVector2D};
use servo::style_traits::DevicePixel;
use servo::script_traits::{TouchEventType,
  MouseButton as ServoMouseButton};
use servo::webrender_api::ScrollLocation;
use servo::keyboard_types::{Code, Key, Modifiers, Location, KeyState,
  KeyboardEvent};

// ------------------------------------------------------------------------

// servo needs some basic resources to run.
// I embed these resources into the binary at compile time

struct ResourceReader;

impl ResourceReader {
  fn new() -> ResourceReader {
    ResourceReader
  }
}

impl ResourceReaderMethods for ResourceReader {
  fn read(&self, res: Resource) -> Vec<u8> {
    macro_rules! inc {
      ($x:expr) => (&include_bytes!(concat!("../resources/", $x))[..]);
    }
    Vec::from(match res {
      Resource::Preferences => inc!("prefs.json"),
      Resource::HstsPreloadList => inc!("hsts_preload.json"),
      Resource::SSLCertificates => inc!("certs"),
      Resource::BadCertHTML => inc!("badcert.html"),
      Resource::NetErrorHTML => inc!("neterror.html"),
      Resource::UserAgentCSS => inc!("user-agent.css"),
      Resource::ServoCSS => inc!("servo.css"),
      Resource::PresentationalHintsCSS => inc!("presentational-hints.css"),
      Resource::QuirksModeCSS => inc!("quirks-mode.css"),
      Resource::RippyPNG => inc!("rippy.png"),
      Resource::DomainList => inc!("public_domains.txt"),
      Resource::BluetoothBlocklist => inc!("gatt_blocklist.txt"),
   })
  }
  fn sandbox_access_files(&self) -> Vec<PathBuf> { vec![] }
  fn sandbox_access_files_dirs(&self) -> Vec<PathBuf> { vec![] }
}

// ------------------------------------------------------------------------

// servo will tell the event loop to wake up once in a while through this

struct GlutinEventLoopWaker {
  proxy: Arc<EventsLoopProxy>,
}

impl EventLoopWaker for GlutinEventLoopWaker {
  fn clone(&self) -> Box<dyn EventLoopWaker + Send> {
    Box::new(GlutinEventLoopWaker{proxy: self.proxy.clone()})
  }

  fn wake(&self) {
    self.proxy.wakeup().expect("(can't wake up)"); // save me
  }
}

// ------------------------------------------------------------------------

struct Window {
  context: glutin::ContextWrapper<glutin::PossiblyCurrent, glutin::Window>,
  waker: Box<dyn EventLoopWaker>,
  gl: Rc<dyn gl::Gl>,
  screen_size: TypedSize2D<u32, DevicePixel>,
  animation_state: Cell<AnimationState>,
}

impl Window {
  pub fn animating(&self) -> bool {
    self.animation_state.get() == AnimationState::Animating
  }
}

// servo calls us through this interface
impl WindowMethods for Window {
  fn prepare_for_composite(&self) -> bool {
    true
  }

  fn present(&self) {
    self.context.swap_buffers().expect("failed to swap buffers");
  }

  fn create_event_loop_waker(&self) -> Box<dyn EventLoopWaker> {
    self.waker.clone()
  }

  fn gl(&self) -> Rc<dyn gl::Gl> {
    self.gl.clone()
  }

  fn set_animation_state(&self, state: AnimationState) {
    self.animation_state.set(state);
  }

  fn get_coordinates(&self) -> EmbedderCoordinates {
    let hdp = self.context.window().get_hidpi_factor();

    let LogicalSize{width, height} = self.context.window()
      .get_outer_size().expect("failed to get outer window size");
    let outer_size = (TypedSize2D::new(width, height) * hdp).to_i32();

    let LogicalPosition{x, y} = self.context.window()
      .get_position().unwrap_or(LogicalPosition::new(0.0, 0.0));
    let origin = (TypedPoint2D::new(x, y) * hdp).to_i32();
    let LogicalSize{width, height} = self.context.window()
      .get_inner_size().expect("failed to get inner window size");
    let viewport = TypedRect::new(
      TypedPoint2D::zero(), // position within window
      (TypedSize2D::new(width, height) * hdp).to_i32(),
    );

    let screen = (self.screen_size.to_f64() * hdp).to_i32();
    EmbedderCoordinates{
      viewport: viewport,
      framebuffer: (TypedSize2D::new(width, height) * hdp).to_i32(),
      window: (outer_size, origin),
      screen: screen,
      screen_avail: screen,
      hidpi_factor: TypedScale::new(1.0),
    }
  }
}

// ------------------------------------------------------------------------

struct Browser {
  servo: Servo<Window>,
  window: Rc<Window>,
  mouse_pos: TypedPoint2D<f64, DevicePixel>,
  drag_start: TypedPoint2D<f64, DevicePixel>,
  drag_button: Option<MouseButton>,
  last_input: Option<KeyboardEvent>,
  event_queue: Vec<ServoWindowEvent>,
}

impl Browser {

// channels of communication:
// - servo calls us through the WindowMethods interface
// - glutin sends us messages (event_loop.run_forever/poll_events callback)
// - servo sends us events (received with servo.get_events())
// - we send events to servo (servo.handle_events)

fn event(&mut self, event: ServoWindowEvent) {
  self.event_queue.push(event);
}

fn mouse_event(&mut self, event: ServoMouseWindowEvent) {
  self.event(ServoWindowEvent::MouseWindowEventClass(event));
}

fn handle_servo_events(&mut self) -> bool {
  let events = self.servo.get_events();
  if events.is_empty() { return false; }
  for (maybe_browser_id, event) in events {
    match event {
      // when you click a url, servo asks if it's allowed to load it
      EmbedderMsg::AllowNavigationRequest(id, _url) => {
        if let Some(_browser_id) = maybe_browser_id {
          self.event(ServoWindowEvent::AllowNavigationResponse(id, true))
        }
      },

      _ => {},
    }
  }
  true
}

fn handle_glutin_event(&mut self, event: Event) {
  let context = &self.window.context;
  let hdp = context.window().get_hidpi_factor();
  match event {
    Event::WindowEvent{event, ..} => match event {
      WindowEvent::Resized(logical_size) => {
        context.resize(logical_size.to_physical(hdp));
        self.event(ServoWindowEvent::Resize);
      },

      WindowEvent::CursorMoved{position: pos, ..} => {
        let PhysicalPosition{x, y} = pos.to_physical(hdp);
        self.mouse_pos = TypedPoint2D::new(x, y);
        self.event(ServoWindowEvent::MouseWindowMoveEventClass(
          self.mouse_pos.to_f32()
        ));
      },

      WindowEvent::MouseWheel{delta, phase, ..} => {
        let hdp32 = hdp as f32;
        let (dx, dy) = match delta {
          MouseScrollDelta::LineDelta(dx, dy) => (dx, dy * 38.0 * hdp32),
          MouseScrollDelta::PixelDelta(position) => {
            let pos = position.to_physical(hdp);
            (pos.x as f32, pos.y as f32)
          },
        };
        let location = ScrollLocation::Delta(TypedVector2D::new(dx, dy));
        let servo_phase = match phase {
          TouchPhase::Started => TouchEventType::Down,
          TouchPhase::Moved => TouchEventType::Move,
          TouchPhase::Ended => TouchEventType::Up,
          TouchPhase::Cancelled => TouchEventType::Cancel,
        };
        self.event(
          ServoWindowEvent::Scroll(location, self.mouse_pos.to_i32(),
            servo_phase)
        );
      },

      WindowEvent::MouseInput{state, button, ..} => {
        let servo_button = match button {
          MouseButton::Left => ServoMouseButton::Left,
          MouseButton::Middle => ServoMouseButton::Middle,
          MouseButton::Right => ServoMouseButton::Right,
          _ => return,
        };
        match state {
          ElementState::Pressed => {
            self.drag_start = self.mouse_pos;
            self.drag_button = Some(button);
            self.mouse_event(
              ServoMouseWindowEvent::MouseDown(servo_button,
                self.mouse_pos.to_f32())
            );
          },
          ElementState::Released => {
            self.mouse_event(
              ServoMouseWindowEvent::MouseUp(servo_button,
                self.mouse_pos.to_f32())
            );
            match self.drag_button {
              Some(btn) if btn == button => {
                // very short drag = click
                let dist =
                  (self.mouse_pos - self.drag_start).square_length();
                if dist < 64.0 * hdp {
                  self.mouse_event(
                    ServoMouseWindowEvent::Click(servo_button,
                      self.mouse_pos.to_f32())
                  );
                }
              },
              _ => {},
            }
          },
        }
      },

      // keyboard handling is a bit tricky. servo expects individual key
      // events for everything, so for printable characters we first
      // store the KeyboardInput event that contains modifiers and so on
      // and then attach the character to it when we get
      // ReceivedCharacter and send the event

      WindowEvent::KeyboardInput{input, ..} => {
        use glutin::VirtualKeyCode::*;
        let ev = KeyboardEvent{
          state: match input.state {
            ElementState::Pressed => KeyState::Down,
            ElementState::Released => KeyState::Up,
          },
          key: match input.virtual_keycode {
            Some(Back) => Key::Backspace,
            Some(Return) => Key::Enter,
            // TODO: handle all non-printable keys
            _ => Key::Unidentified,
          },
          code: match input.scancode {
            // TODO: translate scancode
            _ => Code::Unidentified,
          },
          location: match input.virtual_keycode {
            // TODO: figure out location
            _ => Location::Standard,
          },
          modifiers: Modifiers::empty(), // TODO: translate modifiers
          repeat: false,
          is_composing: false,
        };
        if ev.state == KeyState::Down && ev.key == Key::Unidentified {
          self.last_input = Some(ev);
        } else if ev.key != Key::Unidentified {
          self.last_input = None;
          self.event(ServoWindowEvent::Keyboard(ev));
        }
      },

      WindowEvent::ReceivedCharacter(character) => {
        let mut ch = character;
        if ch.is_control() {
          if ch as u8 >= 32 { return; }
          // convert ascii control characters to letters:
          // ctrl+d types d, ctrl+c types c, and so on
          ch = (ch as u8 + 96) as char;
        }
        let mut event =
          if let Some(input) = mem::replace(&mut self.last_input, None) {
            input
          } else if ch.is_ascii() {
            // non-printable key, already handled by KeyboardInput
            return;
          } else {
            KeyboardEvent::default() // dummy ev for combined characters
          };
        event.key = Key::Character(ch.to_string());
        self.event(ServoWindowEvent::Keyboard(event));
      },

      WindowEvent::CloseRequested => {
        self.event(ServoWindowEvent::Quit);
        // TODO: actually quit
      },

      WindowEvent::Refresh => {
        self.event(ServoWindowEvent::Refresh);
      },

      _ => {},
    },

    Event::Awakened => {
      self.event(ServoWindowEvent::Idle);
    }

    _ => {},
  }
}

fn flush_events(&mut self) {

  // we must make sure all events are flushed. handling servo events can
  // trigger more servo events to be handled and viceversa, so keep looping
  // until everything is resolved

  loop {
    self.servo.handle_events(mem::replace(&mut self.event_queue, vec![]));
    if !self.handle_servo_events() && self.event_queue.is_empty() {
      break;
    }
  }
}

pub fn run() {
  // servo config
  resources::set(Box::new(ResourceReader::new()));
  let args = opts::get(); // defaults
  let winsize = args.initial_window_size.to_f64();

  // init window and opengl context
  let window_builder = glutin::WindowBuilder::new()
    .with_title("scrap")
    .with_dimensions(LogicalSize::new(winsize.width, winsize.height))
    .with_multitouch();
  let mut event_loop = EventsLoop::new();
  let context = glutin::ContextBuilder::new()
    .with_gl(glutin::GlRequest::Specific(glutin::Api::OpenGl, (3, 2)))
    .build_windowed(window_builder, &event_loop)
    .expect("failed to create glutin context");
  let context = unsafe { context.make_current() }
    .expect("failed to make context current");

  // load servo's opengl bindings
  let gl = unsafe {
    GlFns::load_with(|s| context.get_proc_address(s) as *const _)
  };

  let PhysicalSize{width: screen_w, height: screen_h} =
    event_loop.get_primary_monitor().get_dimensions();

  // this will fire Event::Awakened every once in a while
  let waker = Box::new(GlutinEventLoopWaker{
    proxy: Arc::new(event_loop.create_proxy())
  });

  let window = Rc::new(Window{
    context: context,
    waker: waker,
    gl: gl,
    screen_size: TypedSize2D::new(screen_w as u32, screen_h as u32),
    animation_state: Cell::new(AnimationState::Idle),
  });

  let mut browser = Browser{
    servo: Servo::new(window.clone()),
    window: window.clone(),
    mouse_pos: TypedPoint2D::zero(),
    drag_start: TypedPoint2D::zero(),
    drag_button: None,
    last_input: None,
    event_queue: vec![],
  };

  // load first valid url in command line args, or servo.org
  let mut args: Vec<String> = env::args().collect();
  args.push("https://servo.org".to_string());
  for arg in &args[1..] {
    match ServoUrl::parse(arg) {
      Ok(url) => {
        browser.event(ServoWindowEvent::NewBrowser(url, BrowserId::new()));
        break;
      }
      Err(_) => {}
    }
  }

  // if servo is animating, we want to keep polling for events to avoid
  // freezes and delays

  loop {
    if window.animating() {
      event_loop.poll_events(|event| {
        browser.handle_glutin_event(event);
      });
      browser.flush_events();
    } else {
      event_loop.run_forever(|event| {
        use glutin::ControlFlow::*;
        browser.handle_glutin_event(event);
        browser.flush_events();
        if browser.window.animating() {
          // we entered animating state, so start polling events
          Break
        } else {
          Continue
        }
      });
    }
  }
}

} // impl Browser
// ------------------------------------------------------------------------

fn main() {
  Browser::run();
}
