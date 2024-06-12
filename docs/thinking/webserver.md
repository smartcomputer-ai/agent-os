
needed
 - inject messages for actors
 - receive messages from actors (both polling and realtime) 
 - query actor state
 - trees
   - can be specified if enire tree is returend or just the current level



## HTTP Grit
`grit` endpoints are read-only. The runtime does not have to be running.

`$GRIT_URL = http://<host>:<port>/grit/<agent_id|point>`

return all refs  
`GET $GRIT_URL/refs`

return the object with that id  
`GET $GRIT_URL/objects/<object_id>`


## HTTP Wit

`wit` endpoints support interaction with wits via the runtime  

`$WIT_URL http://<host>:<port>/wit/<agent_id|point>`

(NOT NEEDED) get messages sent to the runtime from all actors  
`GET $WIT_URL/messages/`

(NOT NEEDED) get messags sent from a specific actor  
`GET $WIT_URL/messages/<actor_id>`

recieve new message notifications from all actors  
`GET $WIT_URL/messages-sse/`

create a new message for an actor  
`POST $WIT_URL/messages/<actor_id>`

query a wit (running query code in the core of the wit)  
`GET $WIT_URL/query/<actor_id>/<query_name>?query_strings=<query_string>`