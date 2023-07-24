Currently, the system is designed for async. But this will be a mjor shop-stopper for many python developers, since they cannot trivially use their (mainly) sync libraries with the async event loop. (Only a few sync actors can run in parallel because threads are expensive.) Another reason is that LLMs are more likely to write correct sync code than async code.

Therefore, writing sync wits must be possible. But then there is the problem on how to mix the sync and async colored functions. 

The object store and refs now need a sync and async version! Since it is only a handful of methods, that's fine. But what is a bigger challenge are the data model classes, which are also async through and throug and contain longer, intermixed code. To create two versions is a bit of a pain.

-------------------

So, this is a problem of [colored functions](http://journal.stuffwithstuff.com/2015/02/01/what-color-is-your-function/). On the library level, we can just choose to either only support async and let it be up to the user to wrap blocking stuff in a thread or support sync all the way down to the object and reference store...

But, since this is a python lib, sync versions will likely have to be supported, just to not scare away users. 

-------------------

```python
class ObjectStore:
    async def store(self, obj:Object) -> ObjectId:
    async def load(self, object_id:ObjectId) -> Object:

    def store_sync(self, obj:Object) -> ObjectId:
    def load_sync(self, object_id:ObjectId) -> Object:
```

The idea, here, is that the async version is the default, and the sync variant is a consession. Which is the inverse from many other python libs, but, I think, makes sense here.

The more I think about it, the more I'm convinced that this is the way. It's somewaht dirty and, but such is life. Trying to solve this in a "purer" way, might end up with a more elegant solution, but it will be more complex and more difficult to understand. Manually adding _sync variants is a bit of a pain, but it's not too bad. And it's a one-time thing. This is mostly for the higher-level and user-facing apis, the lower level, internal apis will still be async only.

-----
Discussion here:
https://discuss.python.org/t/how-can-async-support-dispatch-between-sync-and-async-variants-of-the-same-code/15014/9
and example solution here 
https://github.com/django/asgiref/blob/d451a724c93043b623e83e7f86743bbcd9a05c45/asgiref/sync.py#L84

Consider this talk on how to build protocol libraries:
https://www.youtube.com/watch?v=7cC3_jGwl_U