import os
import openai
from grit import *
from openai.openai_object import OpenAIObject
from dotenv import load_dotenv
from ..common.messages_chat import ChatMessage

load_dotenv()
openai.api_key = os.getenv("OPENAI_API_KEY")

#chat completion docs
#https://platform.openai.com/docs/api-reference/chat/create

async def chat_completion(messages:list[ChatMessage], 
    system_message:str|None = None, 
    max_tokens:int = 250, 
    temperature:float = 0.6, 
    top_p:float = 1.0,
    actor_id:ActorId|None=None) -> ChatMessage:
    
    # make it work even if the api_key is not set,
    # since this example is used to try the OS for the first time.
    if(openai.api_key is None):
        return ChatMessage.from_actor(
            "I am a helpful assistant, and I want to reply to your question, but the OpenAI API key is not set", 
            actor_id)

    if(system_message is None):
        system_message = "You are a helpful assistant."
    messages_completion = []
    messages_completion.append({
        'role': 'system',
        'content': system_message
    })
    for msg in messages:
        messages_completion.append({
            'role': msg.from_name,
            'content': msg.content
        })
    response:OpenAIObject = await openai.ChatCompletion.acreate(
        model="gpt-3.5-turbo",
        messages=messages_completion,
        max_tokens=max_tokens,
        temperature=temperature,
        top_p=top_p)
    content = response['choices'][0]['message']['content']
    return ChatMessage.from_actor(content, actor_id)
