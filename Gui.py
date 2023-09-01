import stable_whisper
import re
import os
from tkinter import *
from tkinter import (filedialog, messagebox, colorchooser)
import threading
import tkinter as tk
# from tkinter import colorchooser


class VideoEditorApp:
    def __init__(self, root):

        self.root = root
        self.root.title("TranscribVision Alfa 1.0")
        root.geometry('500x550')

        self.max_words_label = tk.Label(root, text="Max words", font=("Helvetica", 13))
        self.max_words_label.place(x=50, y=10)

        self.max_words_label2 = tk.Label(root, text="Max words line", font=("Helvetica", 13))
        self.max_words_label2.place(x=300, y=10)

        #max_words
        self.max_words = tk.Spinbox(root, from_=1, to=100, value=3)
        self.max_words.place(x=50, y=40)

        #max_words_leght
        self.max_chars_leght = tk.Spinbox(root, from_=1, to=100, value=18)
        self.max_chars_leght.place(x=300, y=40)

        self.load_button = Button(root, text="Upload video", command=self.load_video)
        self.load_button.place(x=50, y=90)

        self.activate_button = Button(root, text="Activate script", command=self.activate_script)
        self.activate_button.place(x=50, y=150)

        self.color_display = tk.Label(root, width=10, height=1, bg="white")
        self.color_display.place(x=180, y=120)

        self.color_button = tk.Button(root, text="Choose Color", command=self.choose_color)
        self.color_button.place(x=180, y=90)


        self.label = Label(root, text='')
        self.selected_color = "white"

    def choose_color(self):
        color = colorchooser.askcolor(title="Choose a color")
        if color[1]:  
            self.selected_color = color[1]
            print("Selected color:", color[1])
            self.color_display.config(bg=self.selected_color)

    def load_video(self):
        global file_path
        file_path = filedialog.askopenfilename(filetypes=[("video", "*.mp4 *.avi *.mkv *webm" )])
        if file_path:
            self.label.config(text=f'{file_path}')
            self.label.pack(pady=60)
        else:
            self.label.config(text='')


    def activate_script(self):
        model = stable_whisper.load_model('medium')
        print(f'{file_path} --> Loding...')
        value_word = int(self.max_words.get())
        value_leght = int(self.max_chars_leght.get())
        color_word = self.selected_color
        
        result = model.transcribe(file_path, fp16=False, regroup=True, verbose=True)  
        (
            result
            .split_by_punctuation([('.', ' '), '。', '?', '？', ',', '，'])
            .split_by_gap(.5)
            .merge_by_gap(.10, max_words=value_word)
            .split_by_length(max_words=value_word, max_chars=value_leght)
            
        )
        file_without_extension = file_path.rsplit('.', 1)[0]

        # result.to_srt_vtt(f'{file_without_extension}.srt',word_level=True, tag=('<font color="{}">'.format(color_word), '</font>'))
        result.to_srt_vtt(f'{file_without_extension}.srt', word_level=True, tag=('<font color="{}"><b>'.format(color_word), '</b></font>'))
        result.to_ass(f'{file_without_extension}.ass', word_level=True, tag=('{\\1c&2986cc&}', '{\\r}'))
        result.to_srt_vtt('audio.vtt',word_level=True)

        result.to_ass(f'{file_without_extension}_normal.srt', word_level=False)
        result.to_ass(f'{file_without_extension}_normal.ass', word_level=False)
        result.to_srt_vtt('audio.vtt',word_level=False)

  

if __name__ == "__main__":
    root = Tk()
    app = VideoEditorApp(root)
    root.mainloop()


