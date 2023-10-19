import subprocess
import stable_whisper
import uuid
import requests
import base64
from moviepy.video.io.ffmpeg_tools import ffmpeg_extract_subclip
from moviepy.video.tools.subtitles import SubtitlesClip
from moviepy.editor import *
from moviepy.editor import AudioFileClip, concatenate_audioclips, VideoFileClip
import re
import os
from numba import jit
from tkinter import *
from tkinter import (filedialog, messagebox, colorchooser)
import threading
import tkinter as tk
from tkinter import ttk
import json
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

        self.activate_button = Button(root, text="Subtitle Gerator", command=self.subtitle_gerator)
        self.activate_button.place(x=50, y=150)
    
        self.activate_button = Button(root, text="Merge subtitles", command=self.Merge_subtitles)
        self.activate_button.place(x=50, y=220)

        self.color_display = tk.Label(root, width=10, height=1, bg="red")
        self.color_display.place(x=180, y=120)
        
        # self.color_display_text_color = tk.Label(root, width=10, height=1, bg="white")
        # self.color_display_text_color.place(x=180, y=180)
        
        # self.color_display_selected_color = tk.Label(root, width=10, height=1, bg="black")
        # self.color_display_selected_color.place(x=180, y=250)

        self.movement_color_button = tk.Button(root, text="Choose Color", command=self.choose_color)
        self.movement_color_button.place(x=180, y=90)

        # self.text_color_button = tk.Button(root, text="Color text", command=self.choose_color_text_color)
        # self.text_color_button.place(x=180, y=150)

        # self.text_color_girth_button = tk.Button(root, text="Choose girth", command=self.choose_color_text_color_girth)
        # self.text_color_girth_button.place(x=180, y=220)

        self.file_path=""
        self.file_without_extension=""
        self.label = Label(root, text='')
        
        self.movement_color = "red"
        # self.text_color = "white"
        # self.text_color_girth = "black"

        options_models  = ["base", "small", "medium", "large"]
        self.models  = ttk.Combobox(root, values=options_models)
        self.models.set("medium")
        self.models.place(x=300, y=90)
        


    def choose_color(self):
        color = colorchooser.askcolor(title="Choose a color")
        if color[1]:
            self.movement_color = color[1]
            print("Selected color:", color[1])        
            self.color_display.config(bg=self.movement_color)

    # def choose_color_text_color(self):
    #     color = colorchooser.askcolor(title="Choose a color Text")
    #     if color[1]:
    #         self.text_color = color[1]
    #         print("Selected color:", color[1])
    #         self.color_display_text_color.config(bg=self.text_color)
            

    # def choose_color_text_color_girth(self):
    #     color = colorchooser.askcolor(title="Choose a color Text")
    #     if color[1]:
    #         self.text_color_girth = color[1]
    #         print("Selected color:", color[1])
    #         self.color_display_selected_color.config(bg=self.text_color)

    def load_video(self):
        self.file_path = filedialog.askopenfilename(filetypes=[("video", "*.mp4 *.avi *.mkv *webm *wav *mp3" )])
        if self.file_path:
            self.label.config(text=f'{self.file_path}')
            self.label.pack(pady=60)
        else:
            self.label.config(text='')   
        print(f'Selected: {self.file_path}')

    
    
    def subtitle_gerator (self):
        selected_model_name = self.models.get() 
        print("selected model name:", selected_model_name)
        model = stable_whisper.load_model(selected_model_name)
        self.file_path = self.label.cget("text") 
        if self.file_path:
            print("File path:", self.file_path)
        else:
            print("File path is empty.")

        value_word = int(self.max_words.get())
        value_leght = int(self.max_chars_leght.get())
        color_word = self.movement_color
        print(f'{self.file_path} --> Loding...')
        
        result = model.transcribe(self.file_path, fp16=False, regroup=True, verbose=True)         
                
        (
            result
            .split_by_punctuation([('.', ' '), '。', '?', '？', ',', '，'])
            .split_by_gap(.5)
            .merge_by_gap(.10, max_words=value_word)
            .split_by_length(max_words=value_word, max_chars=value_leght)
            
        )
        
        self.file_without_extension = self.file_path.rsplit('.', 1)[0]

        #    "VTT Default: '<u>', '</u>'"

        result.to_srt_vtt(f'{self.file_without_extension}.srt', word_level=True, tag=('<font color="{}">'.format(color_word), '</font>'))
        
        result.to_srt_vtt(f'{self.file_without_extension}_noColor.srt', word_level=False)
    
        
    def Merge_subtitles(self):
        print('Merging subtitles')
        # text_color = self.text_color
        # text_color_girthr = self.text_color_girth
        
        # print(f'///////////selected_color://///{text_color}')
        # print(f'///////////text_color://///{text_color_girthr}')
        print(f'///////////file_locator/////{self.file_without_extension}')
        print(f'///////////self.file_path/////{self.file_path}')
        
        path_without_drive = os.path.splitdrive(self.file_path)[1]
        file_name_without_extension = os.path.splitext(path_without_drive)[0]
        
        print(f'///////////path_without_drive/////{path_without_drive}')
        print(f'///////////file_name_without_extension/////{file_name_without_extension}')
        
        subtitles_filter = f"subtitles={file_name_without_extension}.srt:force_style='FontName=Lato,Alignment=10,FontSize=28,Bold=1'"  

        ffmpeg_command = f'ffmpeg -y -i "{self.file_path}" -vf "{subtitles_filter}" -c:a copy -c:v libx264 "{file_name_without_extension}_outputSub.mp4"'
        
        print(f'///////////ffmpeg_command////{ffmpeg_command}')
        
        
        subprocess.run(ffmpeg_command, shell=True, check=True) 
        


if __name__ == "__main__":
    root = Tk()
    app = VideoEditorApp(root)
    root.mainloop()


