servo embedding example that I intend to use as a base for a future servo
front-end. this doesn't use libsimpleservo, I haven't looked into it yet
and I'm not sure it's flexible enough.

this is my first ever rust project, and I had only looked at basic examples
before, so expect unconventional code

it handles mouse click and scrolling and basic keyboard input. the only
control codes that are handled are backspace and enter

the resources are copied straight from servo and licensed under the Mozilla
Public License Version 2.0

rest of the code is public domain, see UNLICENSE

# building and running (linux)
install rustup if you don't have it

```sh
curl https://sh.rustup.rs -sSf | sh
```

you might need to add ```~/.cargo/bin/``` to your PATH

install servo dependencies https://github.com/servo/servo#setting-up-your-environment

build and run (this will take ~2GB of disk space)

```
cargo run --release
```

never tried building on other OSes, feel free to contribute your steps

# references
* https://github.com/servo/servo/tree/master/ports/servo
* https://github.com/servo/servo/tree/master/ports/servo/glutin_app
* https://docs.rs/glutin/0.21.0/glutin/
* https://github.com/paulrouget/servo-embedding-example (a bit outdated)
