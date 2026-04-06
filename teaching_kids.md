# Scrcpy for Kids: How it Works! 🚀

Imagine you have a magic window that lets you see and control your phone from your computer. That's **scrcpyrust**! Here's how all the "lego pieces" inside the code work together:

## 1. The Postman (ADB) 📬
Before we can see anything, we need a way to send messages to the phone. The **Postman** (that's the `adb` folder) builds a secret tunnel between your computer and your phone. He delivers a small "Brain" (the `scrcpy-server.jar`) to the phone so it knows how to talk back to us.

## 2. The Movie Camera (Server) 🎥
Once the phone's "Brain" is awake, it starts taking pictures of the screen. But it doesn't just send normal pictures—it squishes them down so they fit through the tunnel really fast. This is like a mini-movie director on your phone!

## 3. The Movie Projector (Display) 📽️
On your computer, the **Movie Projector** (that's `display/screen.rs`) receives those squished pictures. It unsquishes them (with the help of a smart friend named FFmpeg) and shows them on your monitor 60 times every second. That's why it looks so smooth!

## 4. The Remote Control Car (Input) 🏎️
When you click your mouse or type on your keyboard, it's like using a **Remote Control**. The `input/manager.rs` catches your clicks and sends them through the tunnel. The phone's brain receives them and pretends a finger touched the screen!

## 5. The Tape Recorder (Recorder) 📼
Want to save your game to show your friends later? The **Tape Recorder** (that's `media/recorder.rs`) catches the movie as it flies through the tunnel and saves it into a file like a real video camera.

## 6. The Radio (Audio) 📻
Just like a movie needs sound, scrcpyrust has a **Radio** (`audio/player.rs`) that catches the music and sounds from your phone and plays them on your computer's speakers. It even has a special volume knob (`audio/regulator.rs`) that keeps the sound from crackling!

---

### File-by-File Summary
- `main.rs`: The **Conductor** who makes sure everyone starts at the right time.
- `options.rs`: The **Menu** where you choose how big you want the window to be.
- `adb/`: The **Tunnel Digger** who builds the secret path.
- `media/`: The **Translator** who understands the squished pictures.
- `input/`: The **Remote Control** buttons.
- `display/`: The **Screen** you look at!
