import stable_whisper
import re
import os
import pygame
from tkinter import *
from tkinter import filedialog, messagebox
from PIL import Image, ImageTk
import threading
import tkinter as tk


class VideoEditorApp:
    def __init__(self, root):

        self.root = root
        self.root.title("Monotonic Video Editor")
        root.geometry('500x550')

        self.max_words_label = tk.Label(root, text="Max words", font=("Helvetica", 13))
        self.max_words_label.place(x=20, y=10)

        self.max_words_label2 = tk.Label(root, text="Max words line", font=("Helvetica", 13))
        self.max_words_label2.place(x=250, y=10)

        #max_words
        self.max_words = tk.Spinbox(root, from_=1, to=100, value=3)
        self.max_words.place(x=50, y=40)

        #max_words_leght
        self.max_chars_leght = tk.Spinbox(root, from_=1, to=100, value=18)
        self.max_chars_leght.place(x=300, y=40)

        self.load_button = Button(root, text="Wczytaj wideo", command=self.load_video)
        self.load_button.place(x=50, y=90)

        self.activate_button = Button(root, text="Aktywuj skrypt", command=self.activate_script)
        self.activate_button.place(x=50, y=150)

        self.label = Label(root, text='')


    def load_video(self):
        global file_path
        file_path = filedialog.askopenfilename(filetypes=[("video", "*.mp4 *.avi *.mkv *webm" )])
        if file_path:
            self.label.config(text=f'{file_path}')
            self.label.pack(pady=40)
        else:
            self.label.config(text='')


    def activate_script(self):
        model = stable_whisper.load_model('medium')
        print(f'{file_path} --> Loding...')
        value_word = int(self.max_words.get())
        value_leght = int(self.max_chars_leght.get())
        
        result = model.transcribe(file_path, fp16=False, regroup=True, verbose=True)  
        (
            result
            .split_by_punctuation([('.', ' '), '。', '?', '？', ',', '，'])
            .split_by_gap(.5)
            .merge_by_gap(.10, max_words=value_word)
            .split_by_length(max_words=value_word, max_chars=value_leght)
            
        )

        result.to_srt_vtt(f'{file_path}.srt',word_level=True, tag=('<font color="#7519BA">', '</font>'))
        result.to_ass(f'{file_path}.ass', word_level=True, tag=('{\\1c&2986cc&}', '{\\r}'))



if __name__ == "__main__":
    root = Tk()
    app = VideoEditorApp(root)
    root.mainloop()


